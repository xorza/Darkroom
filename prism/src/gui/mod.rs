use std::rc::Rc;
use std::sync::Arc;

use anyhow::Result;
use eframe::NativeOptions;
use egui::{Align, FontId, InnerResponse, Layout, Painter, Rect, Sense, Ui, UiBuilder};

use crate::gui::app::GuiApp;
use crate::{
    common::{StableId, UiEquals},
    gui::style::Style,
};

pub mod app;
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
pub mod image_utils;
pub mod log_ui;
pub mod main_window;
pub mod new_node_ui;
pub mod node_details_ui;
pub mod node_layout;
pub mod node_ui;
pub mod style;
pub mod ui_host;
pub mod value_editor;
pub mod widgets;

/// Entry point for the egui frontend. Spins up eframe with the prism
/// window icon and bundled font, then hands control to [`PrismApp`]
/// in its GUI variant.
pub fn run() -> Result<()> {
    let options = NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_icon(load_window_icon())
            .with_app_id("prism"),
        persist_window: true,
        ..Default::default()
    };

    eframe::run_native(
        "Prism",
        options,
        Box::new(|cc| {
            configure_fonts(&cc.egui_ctx);
            Ok(Box::new(GuiApp::new(&cc.egui_ctx)))
        }),
    )?;

    Ok(())
}

fn load_window_icon() -> Arc<egui::IconData> {
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../../assets/prism.png"))
        .expect("window icon PNG should be a valid RGBA image");
    Arc::new(icon)
}

fn configure_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let font_data = egui::FontData::from_static(include_bytes!("../../assets/Raleway-Medium.ttf"));
    fonts
        .font_data
        .insert("Raleway".to_owned(), Arc::new(font_data));

    let proportional = fonts
        .families
        .get_mut(&egui::FontFamily::Proportional)
        .expect("proportional font family should exist in default font definitions");
    proportional.insert(0, "Raleway".to_owned());

    ctx.set_fonts(fonts);
}

pub struct Gui<'a> {
    ui: &'a mut Ui,
    pub style: Rc<Style>,
    /// Current DPI/zoom scale. `style` is baked at this value; the
    /// invariant is maintained by `new_root` / `child` / `with_scale`
    /// — no other construction path exists.
    scale: f32,
    /// Rect the wrapper was given at construction. Stable within
    /// this `Gui`'s lifetime — callers expect "the rect of this
    /// Gui", not "what's left in the parent Ui after other widgets
    /// rendered." For live queries of the wrapped Ui's remaining
    /// space, use [`Gui::ui_raw`] directly inside widgets.
    pub rect: Rect,
}

impl<'a> Gui<'a> {
    /// Root `Gui` constructor — use at the eframe boundary. `style`
    /// is the reference (at scale=1.0) loaded from TOML; it is
    /// `Rc::clone`d into the wrapper at scale 1.0 and pushed into
    /// egui's global style (one call per frame, not per child `Gui`).
    pub fn new(ui: &'a mut Ui, style: &Rc<Style>) -> Self {
        ui.ctx().global_style_mut(|egui_style| {
            style.apply_to_egui(egui_style);
        });
        Self::child(ui, Rc::clone(style), 1.0)
    }

    /// Build a child `Gui` that inherits `style` and `scale` from its
    /// parent. Only container widgets (inside `gui/`) should call
    /// this — app code gets child `Gui`s through widget closures
    /// (`horizontal`, `Frame::show`, `Panel::show`, …).
    pub(crate) fn child(ui: &'a mut Ui, style: Rc<Style>, scale: f32) -> Self {
        let rect = ui.available_rect_before_wrap();
        Self {
            ui,
            style,
            scale,
            rect,
        }
    }

    /// Raw access to the underlying `egui::Ui`. Restricted to
    /// `gui/widgets/` — every other caller must build a widget rather
    /// than poke `Ui` directly. The `no_raw_ui_outside_widgets` tripwire
    /// test enforces this.
    pub fn ui_raw(&mut self) -> &mut Ui {
        self.ui
    }

    pub fn painter(&self) -> &Painter {
        self.ui.painter()
    }

    /// Load a persisted value (survives app restarts) from
    /// `egui::Memory::data` keyed by `id`. Returns `default` when the
    /// slot is empty.
    pub fn load_persistent<T>(&self, id: egui::Id, default: T) -> T
    where
        T: 'static + Clone + Send + Sync + serde::Serialize + for<'de> serde::Deserialize<'de>,
    {
        self.ui
            .ctx()
            .data_mut(|d| d.get_persisted::<T>(id).unwrap_or(default))
    }

    /// Write a persisted value to `egui::Memory::data` keyed by `id`.
    pub fn store_persistent<T>(&self, id: egui::Id, value: T)
    where
        T: 'static + Clone + Send + Sync + serde::Serialize + for<'de> serde::Deserialize<'de>,
    {
        self.ui.ctx().data_mut(|d| d.insert_persisted(id, value));
    }

    /// Load a temporary value (cleared on app restart) from
    /// `egui::Memory::data` keyed by `id`.
    pub fn load_temp<T: 'static + Clone + Send + Sync>(&self, id: egui::Id) -> Option<T> {
        self.ui.ctx().data_mut(|d| d.get_temp::<T>(id))
    }

    /// Write a temporary value to `egui::Memory::data` keyed by `id`.
    pub fn store_temp<T: 'static + Clone + Send + Sync>(&self, id: egui::Id, value: T) {
        self.ui.ctx().data_mut(|d| d.insert_temp(id, value));
    }

    /// Delete a temporary value of type `T` keyed by `id`.
    pub fn remove_temp<T: 'static + Send + Sync>(&self, id: egui::Id) {
        self.ui.ctx().data_mut(|d| d.remove::<T>(id));
    }

    /// Capture the current frame's input into an [`InputSnapshot`].
    /// Wraps `InputSnapshot::capture(ctx())`.
    pub fn input_snapshot(&self) -> crate::input::InputSnapshot {
        crate::input::InputSnapshot::capture(self.ui.ctx())
    }

    // === egui queries (read-only, not widgets) ===

    pub fn is_rect_visible(&self, rect: Rect) -> bool {
        self.ui.is_rect_visible(rect)
    }

    pub fn rect_contains_pointer(&self, rect: Rect) -> bool {
        self.ui.rect_contains_pointer(rect)
    }

    pub fn pointer_hover_pos(&self) -> Option<egui::Pos2> {
        self.ui.input(|i| i.pointer.hover_pos())
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Internal — the only legitimate scale transition is
    /// [`with_scale`], which saves/restores automatically. Clones
    /// the reference `Style` at the new scale via [`Style::at_scale`];
    /// the reference back-link inside the current `Rc<Style>` means
    /// we always multiply from canonical scale=1.0 values.
    fn set_scale(&mut self, scale: f32) {
        assert!(scale.is_finite(), "gui scale must be finite");
        assert!(scale > 0.0, "gui scale must be greater than 0");
        if self.scale.ui_equals(scale) {
            return;
        }
        self.scale = scale;
        self.style = self.style.at_scale(scale);
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
            let mut gui = Gui::child(ui, style, scale);
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
                let mut gui = Gui::child(ui, style, scale);
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
            let mut gui = Gui::child(ui, style, scale);
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
            clip_rect: None,
        }
    }

    /// Runs a closure with a temporarily changed scale. Saves the
    /// current `Rc<Style>` + scale and restores them by assignment
    /// after the closure returns — no second rebuild on the restore
    /// path.
    pub fn with_scale<R>(&mut self, scale: f32, f: impl FnOnce(&mut Gui<'_>) -> R) -> R {
        let prev_style = Rc::clone(&self.style);
        let prev_scale = self.scale;
        self.set_scale(scale);
        let result = f(self);
        self.style = prev_style;
        self.scale = prev_scale;
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
    clip_rect: Option<Rect>,
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

    /// Narrows the child scope's clip rect. Anything the body paints
    /// outside this rect is culled.
    pub fn clip_rect(mut self, rect: Rect) -> Self {
        self.clip_rect = Some(rect);
        self
    }

    pub fn show<R>(self, add_contents: impl FnOnce(&mut Gui<'_>) -> R) -> R {
        let style = Rc::clone(&self.gui.style);
        let scale = self.gui.scale;
        let clip_rect = self.clip_rect;
        // Delegate to `scope_builder`, NOT `Ui::new_child` directly: only
        // `scope_builder` calls `remember_min_rect` + `advance_cursor_after_rect`
        // when the closure returns. Without those the parent's cursor never
        // moves past us, so siblings stack on top of each other and inner
        // layouts (Frame, horizontal, etc.) render at the wrong position.
        self.gui
            .ui
            .scope_builder(self.builder, |ui| {
                if let Some(clip) = clip_rect {
                    ui.set_clip_rect(clip);
                }
                let mut gui = Gui::child(ui, style, scale);
                add_contents(&mut gui)
            })
            .inner
    }
}
