use std::rc::Rc;
use std::sync::Arc;

use egui::{
    Color32, FontId, Galley, InnerResponse, Layout, Painter, Rect, Response, Sense, Ui, UiBuilder,
    Vec2,
};

use crate::common::{StableId, UiEquals};
use crate::gui::memory::Memory;
use crate::gui::style::Style;

pub mod app;
pub mod bootstrap;
pub mod debug;
pub mod graph_ui;
pub mod image_utils;
pub mod log_ui;
pub mod main_window;
pub mod memory;
pub mod settings_window;
pub(super) mod shortcuts;
pub mod style;
pub mod ui_host;
pub mod value_editor;
pub mod widgets;

pub use bootstrap::run;

/// Read-only bundle of frame-constant view parameters: style, scale,
/// rect. Built from a `Gui<'_>` via [`Gui::view_params`]. Pure-geometry
/// helpers (layout maths, zoom-target computation) take `&ViewParams`
/// instead of `&Gui<'_>` so they can be unit-tested without an
/// `egui::Ui`. The `Rc<Style>` is cheap-cloned at construction so the
/// caller is free to re-borrow `gui` mutably while a `ViewParams` is
/// live.
#[derive(Debug, Clone)]
pub struct ViewParams {
    pub style: Rc<Style>,
    pub scale: f32,
    pub rect: Rect,
}

impl ViewParams {
    /// Enter an egui container's body closure: wrap the fresh `&mut Ui`
    /// egui hands back as a child `Gui<'_>` carrying our inheritance
    /// state (`style`, `scale`). The snapshot's `rect` is ignored —
    /// `Gui::child` reads `available_rect_before_wrap` from the new
    /// `Ui`, so the child sees the container's actual rect, not the
    /// parent's. This is the single sanctioned path for container
    /// widgets in `gui/widgets/` to construct child `Gui`s.
    pub(crate) fn enter<R>(self, ui: &mut Ui, body: impl FnOnce(&mut Gui<'_>) -> R) -> R {
        let mut child = Gui::child(ui, self.style, self.scale);
        body(&mut child)
    }
}

pub struct Gui<'a> {
    ui: &'a mut Ui,
    pub style: Rc<Style>,
    /// Current DPI/zoom scale. `style` is baked at this value; the
    /// invariant is maintained by `new_root` / `child` / `with_scale`
    /// — no other construction path exists.
    scale: f32,
    /// Available rect snapshotted at construction. Read via
    /// [`Gui::container_rect`]; see that doc for semantics.
    pub(crate) rect: Rect,
}

impl<'a> Gui<'a> {
    /// Root `Gui` constructor — use at the eframe boundary. `style`
    /// is the reference (at scale=1.0); it is `Rc::clone`d into the
    /// wrapper at scale 1.0. `Style::apply_to_egui` is *not* called
    /// here — `GuiApp::new` pushes it into egui's global style once at
    /// app init.
    pub fn new(ui: &'a mut Ui, style: &Rc<Style>) -> Self {
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

    /// Persistence view over `egui::Memory::data` keyed by [`StableId`].
    pub fn memory(&self) -> Memory<'_> {
        Memory::new(self.ui.ctx())
    }

    /// Layout text into an `Arc<Galley>` (no wrapping). Thin wrapper over
    /// [`egui::Painter::layout_no_wrap`] that takes references — egui's
    /// API consumes `String` and `FontId`, so callers were repeating
    /// `text.to_string()` / `font.clone()` boilerplate. Lookup goes
    /// through egui's internal `GalleyCache`; the actual layout work is
    /// amortized across frames for unchanged inputs.
    pub fn layout_no_wrap(&self, text: &str, font: &FontId, color: Color32) -> Arc<Galley> {
        self.ui
            .painter()
            .layout_no_wrap(text.to_owned(), font.clone(), color)
    }

    /// Mark `id`'s `Order::Middle` layer as the modal-input layer for
    /// this frame. egui blocks focus traversal into anything below it.
    pub fn set_modal_layer(&self, id: StableId) {
        let layer = egui::LayerId::new(egui::Order::Middle, id.id());
        self.ui.ctx().memory_mut(|mem| mem.set_modal_layer(layer));
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

    pub fn pointer_hover_pos(&self) -> Option<egui::Pos2> {
        self.ui.input(|i| i.pointer.hover_pos())
    }

    pub fn rect_contains_pointer(&self, rect: Rect) -> bool {
        self.ui.rect_contains_pointer(rect)
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Rect this `Gui` was constructed with — `available_rect_before_wrap`
    /// of the parent `Ui` at the moment of construction, frozen for the
    /// lifetime of this wrapper. **Not** the parent's live remaining
    /// rect: it does not shrink as siblings render. Container widgets
    /// inside `gui/widgets/` that need the live cursor should reach for
    /// `ui_raw()` instead.
    pub fn container_rect(&self) -> Rect {
        self.rect
    }

    /// Read-only frame-constant view parameters: style, scale, rect.
    /// Pure-geometry helpers take `&ViewParams` instead of `&Gui<'_>`
    /// so they can be unit-tested without an `egui::Ui`. Cheap: one
    /// `Rc::clone` plus two field copies.
    pub fn view_params(&self) -> ViewParams {
        ViewParams {
            style: Rc::clone(&self.style),
            scale: self.scale,
            rect: self.rect,
        }
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

    /// Pin the current layout's minimum height — siblings allocate
    /// at least `height` on the cross axis. Form rows use this to
    /// guarantee labels/text-edits/buttons share a baseline. Sanctioned
    /// thin wrapper over `Ui::set_min_height` so app code doesn't
    /// reach for `ui_raw()`.
    pub fn set_min_height(&mut self, height: f32) {
        self.ui.set_min_height(height);
    }

    pub fn horizontal<R>(
        &mut self,
        add_contents: impl FnOnce(&mut Gui<'_>) -> R,
    ) -> InnerResponse<R> {
        let args = self.view_params();
        self.ui.horizontal(|ui| args.enter(ui, add_contents))
    }

    /// Run `add_contents` under an arbitrary [`egui::Layout`]. Generic
    /// escape hatch for layouts not covered by `horizontal` / `vertical`
    /// — e.g. cross-axis-justified rows, right-to-left, top-down with
    /// custom alignment.
    pub fn with_layout<R>(
        &mut self,
        layout: Layout,
        add_contents: impl FnOnce(&mut Gui<'_>) -> R,
    ) -> InnerResponse<R> {
        let args = self.view_params();
        self.ui
            .with_layout(layout, |ui| args.enter(ui, add_contents))
    }

    pub fn vertical<R>(
        &mut self,
        add_contents: impl FnOnce(&mut Gui<'_>) -> R,
    ) -> InnerResponse<R> {
        let args = self.view_params();
        self.ui.vertical(|ui| args.enter(ui, add_contents))
    }

    /// Single form line: pinned to `style.row_height` with cross-axis
    /// `Align::Center`, so labels, text edits, and buttons drawn in
    /// the same row share a vertical centerline regardless of their
    /// natural heights. Use for any "label + input" row.
    pub fn form_row<R>(&mut self, add_contents: impl FnOnce(&mut Gui<'_>) -> R) -> R {
        let row_height = self.style.row_height;
        self.with_layout(Layout::left_to_right(egui::Align::Center), |gui| {
            gui.set_min_height(row_height);
            add_contents(gui)
        })
        .inner
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

    /// Allocate `size` in the current layout under this scope and
    /// return `(rect, response)`. Counterpart to
    /// [`HitRegion::interact_and_cull`] for caller-supplied rects: any
    /// widget that does its own layout based on content size (Button,
    /// future autosize widgets) should go through this so the
    /// `allocate_*` calls live behind a single banned-pattern whitelist
    /// and not at every call site. Sets the scope's interaction sense
    /// to `sense` automatically.
    pub fn autosize(self, size: Vec2, sense: Sense) -> (Rect, Response) {
        self.sense(sense).show(|gui| {
            let (_id, rect) = gui.ui_raw().allocate_space(size);
            let response = gui.ui_raw().allocate_rect(rect, sense);
            (rect, response)
        })
    }

    pub fn show<R>(self, add_contents: impl FnOnce(&mut Gui<'_>) -> R) -> R {
        let args = self.gui.view_params();
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
                args.enter(ui, add_contents)
            })
            .inner
    }
}
