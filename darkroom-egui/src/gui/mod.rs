use std::rc::Rc;
use std::sync::Arc;

use egui::{
    Align, Color32, Direction, FontId, Galley, InnerResponse, Layout, Painter, Rect, Response,
    Sense, Ui, UiBuilder, Vec2, vec2,
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

    /// Indented vertical sub-section with a left vline. Mirrors
    /// `egui::Ui::indent` but uses our own indent width
    /// ([`style.indent`](Style::indent)) and stroke
    /// (`style.inactive_bg_stroke`). Caller must already be in a
    /// vertical layout — that's the only place a left-indent makes
    /// sense.
    pub fn indent<R>(&mut self, id: StableId, body: impl FnOnce(&mut Gui<'_>) -> R) -> R {
        let indent = self.style.indent;
        let stroke = self.style.inactive_bg_stroke;

        let mut child_rect = self.ui.available_rect_before_wrap();
        child_rect.min.x += indent;

        let line_x = child_rect.min.x - indent * 0.5;
        let line_top_y = child_rect.min.y;

        let result = self.scope(id).max_rect(child_rect).show(body);

        // After the scope ends, the parent cursor has advanced past
        // the indented content. Trim a hair so the line doesn't bleed
        // into the next sibling row.
        let line_bottom_y = self.ui.cursor().min.y - self.style.small_padding;
        self.painter()
            .vline(line_x, line_top_y..=line_bottom_y, stroke);

        result
    }

    /// Single form line: explicit `(available_width, row_height)`
    /// allocation with cross-axis `Align::Center`, so labels, text
    /// edits, and buttons drawn in the same row share a vertical
    /// centerline regardless of their natural heights. Equivalent to
    /// [`Gui::row_with_layout`] with `Layout::left_to_right(Align::Center)`.
    pub fn form_row<R>(&mut self, add_contents: impl FnOnce(&mut Gui<'_>) -> R) -> R {
        self.row_with_layout(Layout::left_to_right(egui::Align::Center), add_contents)
    }

    /// Row pinned to `style.row_height` under `layout`, with width
    /// either filling the parent (normal pass) or driven by children
    /// (sizing pass). Use when you need a non-default direction (e.g.
    /// `Layout::right_to_left(Center)` for a footer with Apply/Cancel
    /// on the right).
    ///
    /// Height pin: egui's horizontal layouts with `cross_align ==
    /// Center` trip a "fill cross axis to `available_rect.height()`"
    /// branch in `Layout::next_frame`. When the parent's
    /// `max_rect.height` is bigger than the row content — which
    /// happens whenever `egui::Window`'s `Resize` keeps `desired_size`
    /// pinned at its high-water mark — every widget in the row would
    /// claim that full height and the cursor would advance by it, so
    /// the row swallows all remaining vertical space and pushes later
    /// siblings off the visible area. A fixed-size allocation gives
    /// the row a finite `max_rect.height`, so the fill becomes
    /// `row_height`.
    ///
    /// Width: in the normal pass we claim `available_width` so
    /// right-to-left / center layouts have slack to align into. In a
    /// sizing pass (`ui.is_sizing_pass()`, triggered by `Area` /
    /// `Window::auto_sized()` on the first frame state is unknown) we
    /// allocate width 0 and let egui's allocator grow `min_rect` to
    /// fit children — so the parent measures *content* width, not
    /// INFINITY. Same trick `egui::Sides` (sides.rs:163) and `Grid`
    /// use to play nicely with auto-sized parents.
    pub fn row_with_layout<R>(
        &mut self,
        layout: Layout,
        add_contents: impl FnOnce(&mut Gui<'_>) -> R,
    ) -> R {
        let row_height = self.style.row_height;
        let args = self.view_params();
        let width = if self.ui.is_sizing_pass() {
            0.0
        } else {
            self.ui.available_size_before_wrap().x
        };
        let desired_size = egui::vec2(width, row_height);
        self.ui
            .allocate_ui_with_layout(desired_size, layout, |ui| args.enter(ui, add_contents))
            .inner
    }

    /// Allocate exactly `size` in the current layout and return its
    /// rect + response. The caller is responsible for being inside a
    /// stable-id scope (`Gui::scope(...).show(...)` body, container
    /// widget body, etc.) so the auto-id seeded by `allocate_exact_size`
    /// does not drift with the parent's widget counter.
    ///
    /// Lives behind the `gui/mod.rs` banned-pattern whitelist so callers
    /// don't need a per-line `// id-drift-ok` annotation.
    pub fn allocate_exact_size(&mut self, size: Vec2, sense: Sense) -> (Rect, Response) {
        self.ui.allocate_exact_size(size, sense)
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

    /// Row with up to three anchored slots — `left`, `center`,
    /// `right` — that lays out correctly inside an autosizing
    /// container (modal, popup, autosizing frame). Row height is
    /// `style.row_height`; widgets taller than that pin the row to
    /// their own height via the usual `min_rect` growth, so passing
    /// the height explicitly is rarely useful.
    ///
    /// Each slot's body runs exactly once. During a sizing pass all
    /// bodies stack left-to-right at their natural sizes inside one
    /// scope, so the row's reported `min_rect` is the sum of natural
    /// widths — that's what the parent `Area` measures and stores as
    /// its size. During the visible pass the row is pinned to
    /// `(available_width, row_height)` and each slot opens its own
    /// scope over the shared `row_rect` with the appropriate layout
    /// (LTR / `centered_and_justified` / RTL), giving a centered
    /// child and edge-anchored siblings without inflating the parent.
    ///
    /// The slot scopes use `Sense::empty()` so only the slot bodies'
    /// own widgets register pointer interactions — empty title-bar
    /// space falls through to the enclosing `Area`'s drag handle.
    /// Pass `Label::selectable(false)` for chrome labels inside slot
    /// bodies; a selectable label registers `click_and_drag` over
    /// its galley and would intercept drags.
    pub fn row_slots(&mut self, id: StableId, build: impl FnOnce(&mut RowSlots<'_, '_>)) {
        if self.ui.is_sizing_pass() {
            self.scope(id.with("sizing"))
                .layout(Layout::left_to_right(Align::Center))
                .show(|gui| {
                    let mut slots = RowSlots {
                        gui,
                        id,
                        visible_rect: None,
                    };
                    build(&mut slots);
                });
        } else {
            let height = self.style.row_height;
            let width = self.ui.available_width();
            let row_rect = self.scope(id.with("row")).allocate(vec2(width, height));
            let mut slots = RowSlots {
                gui: self,
                id,
                visible_rect: Some(row_rect),
            };
            build(&mut slots);
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

    /// Override the scope's layout. Used to overlay multiple layouts
    /// on the same rect — e.g. a title bar that needs one child
    /// centered and another anchored to the right edge: allocate the
    /// row, then `scope(...).max_rect(row).layout(centered_and_justified)`
    /// for the title and `scope(...).max_rect(row).layout(right_to_left)`
    /// for the close button. The two scopes don't interact.
    pub fn layout(mut self, layout: Layout) -> Self {
        self.builder = self.builder.layout(layout);
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

    /// Like [`Self::autosize`] but skips the interaction registration:
    /// the rect is reserved in the parent's layout and returned for
    /// caller use (painting, sub-scopes via `.max_rect(..)`), but no
    /// hover/click/drag widget is created. Use this when the rect is
    /// pure layout scaffolding — e.g. a title bar shell that lets
    /// the parent `Area`'s drag handle reach pointer events on its
    /// empty space.
    pub fn allocate(self, size: Vec2) -> Rect {
        self.sense(Sense::empty()).show(|gui| {
            let (_id, rect) = gui.ui_raw().allocate_space(size);
            rect
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

/// Builder passed to [`Gui::row_slots`]'s body closure. Each method
/// runs its slot's body once: in the sizing pass all slots run
/// sequentially (LTR, natural sizes); in the visible pass each runs
/// inside its own overlaid scope on the shared row rect.
pub struct RowSlots<'b, 'a> {
    gui: &'b mut Gui<'a>,
    id: StableId,
    visible_rect: Option<Rect>,
}

impl<'b, 'a> RowSlots<'b, 'a> {
    /// Anchored at the row's left edge (LTR layout). No live caller
    /// yet — kept for API symmetry with [`Self::center`] /
    /// [`Self::right`]; remove if a year passes without one appearing.
    #[allow(dead_code)]
    pub fn left(&mut self, body: impl FnOnce(&mut Gui<'_>)) {
        self.run(SlotKind::Left, body);
    }

    pub fn center(&mut self, body: impl FnOnce(&mut Gui<'_>)) {
        self.run(SlotKind::Center, body);
    }

    pub fn right(&mut self, body: impl FnOnce(&mut Gui<'_>)) {
        self.run(SlotKind::Right, body);
    }

    fn run(&mut self, kind: SlotKind, body: impl FnOnce(&mut Gui<'_>)) {
        match self.visible_rect {
            None => {
                // Sizing pass: stack LTR at natural sizes so the
                // parent measures the row's full width.
                body(self.gui);
            }
            Some(rect) => {
                let (layout, salt) = match kind {
                    SlotKind::Left => (Layout::left_to_right(Align::Center), "left"),
                    SlotKind::Center => (
                        Layout::centered_and_justified(Direction::LeftToRight),
                        "center",
                    ),
                    SlotKind::Right => (Layout::right_to_left(Align::Center), "right"),
                };
                self.gui
                    .scope(self.id.with(salt))
                    .max_rect(rect)
                    .sense(Sense::empty())
                    .layout(layout)
                    .show(body);
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum SlotKind {
    #[allow(dead_code)]
    Left,
    Center,
    Right,
}
