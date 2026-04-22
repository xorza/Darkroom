use std::rc::Rc;

use egui::{Align, FontId, InnerResponse, Layout, Painter, Rect, Sense, Ui, UiBuilder};

use crate::{
    common::{StableId, UiEquals},
    gui::style::Style,
};

pub mod connection_bezier;
pub mod connection_breaker;
pub mod connection_ui;
pub mod const_bind_ui;
pub mod frame_output;
pub mod gesture;
pub mod graph_background;
pub mod graph_ctx;
pub mod graph_layout;
pub mod graph_ui;
pub mod log_ui;
pub mod new_node_ui;
pub mod node_details_ui;
pub mod node_layout;
pub mod node_ui;
pub mod style;
pub mod style_settings;

pub struct Gui<'a> {
    ui: &'a mut Ui,
    pub style: Rc<Style>,
    pub rect: Rect,
    scale: f32,
}

impl<'a> Gui<'a> {
    pub fn new(ui: &'a mut Ui, style: &Rc<Style>) -> Self {
        Self::with_style(ui, Rc::clone(style), 1.0)
    }

    pub fn new_with_scale(ui: &'a mut Ui, style: &Rc<Style>, scale: f32) -> Self {
        Self::with_style(ui, Rc::clone(style), scale)
    }

    /// Build a `Gui` that takes ownership of a cloned `Rc<Style>`. Prefer this
    /// in hot paths — it avoids the outer-then-inner double Rc bump that
    /// `new_with_scale` produces.
    fn with_style(ui: &'a mut Ui, style: Rc<Style>, scale: f32) -> Self {
        let rect = ui.available_rect_before_wrap();
        Self {
            ui,
            style,
            rect,
            scale,
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

    /// Internal — the only legitimate scale transition is `with_scale`,
    /// which saves/restores automatically. External callers build a
    /// child `Gui` at the desired scale via `Gui::new_with_scale`.
    fn set_scale(&mut self, scale: f32) {
        assert!(scale.is_finite(), "gui scale must be finite");
        assert!(scale > 0.0, "gui scale must be greater than 0");

        self.scale = scale;

        if self.style.scale.ui_equals(scale) {
            return;
        }

        // `Rc::make_mut` mutates in place when refcount == 1 (the normal
        // case inside `with_scale`: no child `Gui`s exist yet). If a
        // child is still alive, this allocates a scaled clone — also
        // correct, just not free.
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
        let scale = self.scale;
        self.ui.horizontal(|ui| {
            let mut gui = Gui::with_style(ui, style, scale);
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
        let scale = self.scale;
        self.ui.with_layout(
            Layout::left_to_right(Align::Min).with_cross_justify(true),
            |ui| {
                let mut gui = Gui::with_style(ui, style, scale);
                add_contents(&mut gui)
            },
        )
    }

    pub fn vertical<R>(
        &mut self,
        add_contents: impl FnOnce(&mut Gui<'_>) -> R,
    ) -> InnerResponse<R> {
        let style = Rc::clone(&self.style);
        let scale = self.scale;
        self.ui.vertical(|ui| {
            let mut gui = Gui::with_style(ui, style, scale);
            add_contents(&mut gui)
        })
    }

    /// Begin a stable-id child scope. Returns a builder that applies
    /// optional `.max_rect(..)` / `.sense(..)` tweaks and finishes with
    /// `.show(|gui| ...)`, which creates the child `Gui` whose widget
    /// id equals `id.id()` verbatim (`global_scope=true`) — **not**
    /// `parent.id.with(salt).with(parent_counter)` like egui's default
    /// `UiBuilder::id_salt` produces. That distinction shields our
    /// chrome from "widget rect changed id between passes" warnings
    /// when adjacent conditional siblings come and go.
    pub fn scope(&mut self, id: StableId) -> ScopedGui<'_, 'a> {
        ScopedGui {
            gui: self,
            builder: UiBuilder::new().id(id.id()),
        }
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

/// Builder returned by [`Gui::scope`]. Accumulates optional rect/sense
/// tweaks, then runs the body under a child `Gui` whose widget id is
/// pinned to the caller-supplied [`StableId`]. See `Gui::scope` for
/// why the id-stability matters.
#[must_use = "ScopedGui does nothing until .show() is called"]
pub struct ScopedGui<'b, 'a> {
    gui: &'b mut Gui<'a>,
    builder: UiBuilder,
}

impl<'b, 'a> ScopedGui<'b, 'a> {
    pub fn max_rect(mut self, rect: Rect) -> Self {
        self.builder = self.builder.max_rect(rect);
        self
    }

    pub fn sense(mut self, sense: Sense) -> Self {
        self.builder = self.builder.sense(sense);
        self
    }

    pub fn show<R>(self, add_contents: impl FnOnce(&mut Gui<'_>) -> R) -> R {
        let style = Rc::clone(&self.gui.style);
        let scale = self.gui.scale;
        // Delegate to `scope_builder`, NOT `Ui::new_child` directly: only
        // `scope_builder` calls `remember_min_rect` + `advance_cursor_after_rect`
        // when the closure returns. Without those the parent's cursor never
        // moves past us, so siblings stack on top of each other and inner
        // layouts (Frame, horizontal, etc.) render at the wrong position.
        self.gui
            .ui
            .scope_builder(self.builder, |ui| {
                let mut gui = Gui::with_style(ui, style, scale);
                add_contents(&mut gui)
            })
            .inner
    }
}
