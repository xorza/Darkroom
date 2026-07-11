//! Full-resolution viewers for ports' cached runtime images, one editor
//! tab per port ([`TabRef::ImageViewer`], deduped on open). Clicking an
//! inspector-panel thumbnail hands the port's held [`DynamicValue`] over
//! (the full value the worker already delivered for the open panel — an
//! `Arc` clone, so no recompute and no worker round-trip) and focuses the
//! port's tab; if the value is gone (superseded run), the pane says so.
//!
//! **Content follows the graph**: each viewer remembers the run epoch its
//! image came from, and `Editor::sync_image_viewers` re-presents the
//! port's freshly fetched value after every run (viewer-tab nodes ride the
//! same worker value-fetch as open inspector panels), keeping the user's
//! pan/zoom when the dimensions are unchanged. A value the run reused
//! (same `Arc`) skips the re-render entirely. A restored (or undo-
//! reopened) tab starts empty and fills itself the same way.
//!
//! The heavy work — RGBA8 conversion of the full buffer and the texture
//! upload — happens lazily on the first draw after a (re)present, since
//! only the record pass holds the `Ui`. The buffer is CPU-resident by
//! construction: the worker's thumbnail generation already pulled it to
//! the CPU in place (`ImageBuffer` interior mutability).
//!
//! [`TabRef::ImageViewer`]: crate::core::document::TabRef::ImageViewer

use aperture::{
    Align, Background, Color, Configure, Corners, HAlign, ImageFilter, ImageFit, ImageHandle,
    Panel, PointerButton, Rect, Sense, Shape, Size, Sizing, Spacing, Stroke, Text, TextStyle, Ui,
    VAlign, WidgetId,
};
use glam::{UVec2, Vec2};
use imaginarium::{ColorFormat, Preview, ProcessingContext};
use lens::Image as LensImage;
use scenarium::data::DynamicValue;
use scenarium::graph::NodeSearch;

use crate::core::document::{Document, PortKind, PortRef};
use crate::core::worker::RunId;
use crate::gui::canvas::pan_zoom::{scroll_to_zoom_factor, zoom_about};
use crate::gui::theme::Theme;
use crate::gui::widgets::toolbar::{
    BUTTON_GAP, Chip, TOOLBAR_MARGIN, pill, pill_background, pill_rule,
};

/// Longest texture side we upload. Conservative device
/// `max_texture_dimension_2d` (aperture doesn't expose the real limit);
/// larger images are area-downscaled to fit and the header says so.
const MAX_TEXTURE_DIM: usize = 8192;

/// Viewer zoom bounds — far wider than the canvas's: out to overview a
/// texture-capped 8k frame in a small pane, in for pixel peeping.
const MIN_ZOOM: f32 = 0.02;
const MAX_ZOOM: f32 = 32.0;

/// On-screen side of one checkerboard square, logical px. Screen-fixed
/// (doesn't pan/zoom with the image) — it's a transparency reference,
/// not content.
const CHECKER_SQUARE_PX: f32 = 8.0;

/// Backdrop behind (and around) the image. Runtime-only viewer state,
/// like the pan/zoom framing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ViewerBackground {
    /// The editor's canvas fill — the resting default.
    #[default]
    Theme,
    Black,
    White,
    /// Neutral gray checkerboard — the transparency reference.
    Checker,
}

/// One image-viewer tab's state: what it shows and how it's framed.
/// Lives in the `MainWindow`'s per-port viewer map, keyed by (and
/// carrying) the [`PortRef`] its tab binds to; content is runtime-only
/// (never persisted).
#[derive(Debug)]
pub(crate) struct ImageViewer {
    /// The port this viewer shows — keys the pane's widget id so two
    /// viewer tabs never share gesture responses.
    port: PortRef,
    /// Header label (node name + port tag), refreshed on each present.
    title: String,
    /// Value handed over by present/refresh, converted + uploaded on the
    /// next draw (which has the `Ui` to register the texture).
    pending: Option<DynamicValue>,
    /// The value currently shown (or converting), kept so a refresh with
    /// the same `Arc` (node reused its cache across the run) can skip the
    /// re-render.
    shown: Option<DynamicValue>,
    image: Option<ImageHandle>,
    /// Source dimensions before the texture-cap downscale.
    native_size: UVec2,
    /// Source pixel format before the RGBA8 view conversion, for the
    /// header readout; `None` while nothing is shown.
    native_format: Option<ColorFormat>,
    /// Why the pane is empty, when it is.
    message: Option<String>,
    /// Run epoch of the current content (`shown`/message), `None` while
    /// nothing from any run is shown — so the first fetched value always
    /// fills the pane (e.g. a tab restored from a saved document).
    content_epoch: Option<RunId>,
    /// Explicit viewport once the user pans/zooms; `None` = fit-to-pane
    /// (recomputed each frame, so it tracks pane resizes).
    view: Option<ViewerViewport>,
    /// Pan-drag bookkeeping: the viewport pan at drag start.
    pan_anchor: Option<Vec2>,
    /// Backdrop mode, picked from the control panel.
    background: ViewerBackground,
    /// Texel sampling for the shown image, toggled from the control
    /// panel. Starts at `Nearest` — hard texels for pixel peeping;
    /// `Linear` smooths.
    filter: ImageFilter,
    /// Lazily registered checkerboard tile for the `Checker` backdrop.
    checker: Option<ImageHandle>,
}

/// The viewer's viewport: the image's top-left offset in pane-local px
/// plus the zoom factor (pane px per texture texel) — the same
/// `local = pan + zoom * texel` mapping as the canvas viewport, so the
/// shared zoom-about-cursor algebra applies unchanged.
#[derive(Clone, Copy, Debug)]
struct ViewerViewport {
    pan: Vec2,
    zoom: f32,
}

/// An RGBA8 render of a full image value, ready to register, plus the
/// source dimensions and pixel format it was derived from.
#[derive(Debug)]
struct RenderedImage {
    image: aperture::Image,
    native_size: UVec2,
    native_format: ColorFormat,
}

impl ImageViewer {
    /// An empty viewer for `port` (shows the hint until content arrives).
    pub(crate) fn new(port: PortRef) -> Self {
        Self {
            port,
            title: String::new(),
            pending: None,
            shown: None,
            image: None,
            native_size: UVec2::ZERO,
            native_format: None,
            message: None,
            content_epoch: None,
            view: None,
            pan_anchor: None,
            background: ViewerBackground::default(),
            filter: ImageFilter::Nearest,
            checker: None,
        }
    }

    /// Back to fit-to-pane framing (and cancel any pan in progress) —
    /// shared by present, the fit button, double-click, and a
    /// dimensions-changed refresh.
    fn reset_framing(&mut self) {
        self.view = None;
        self.pan_anchor = None;
    }

    /// Show `value` as this viewer's content, resetting the framing to
    /// fit — the preview-click path. `value` is the port's held runtime
    /// value from `epoch` (`None` when it's no longer cached, which shows
    /// a message and leaves the epoch unset so the next run's value still
    /// fills the pane).
    pub(crate) fn present(&mut self, title: String, value: Option<DynamicValue>, epoch: RunId) {
        self.reset_framing();
        self.set_content(title, value, epoch);
        // A missing value on an explicit click is a transient condition
        // (mid-run, superseded epoch): stay epoch-less so the run's
        // arriving value refreshes the pane.
        if self.shown.is_none() {
            self.content_epoch = None;
            self.message =
                Some("value is no longer cached — it will appear after the next run".to_owned());
        }
    }

    /// Update the content to `epoch`'s fetched `value`, keeping the user's
    /// framing — the after-run path. No-op when this epoch is already
    /// shown; a value that is the same `Arc` as the current one (the node
    /// reused its cache) just adopts the epoch without re-rendering.
    /// `value` is `None` when the port has no image value this run.
    pub(crate) fn refresh(&mut self, title: String, value: Option<DynamicValue>, epoch: RunId) {
        if self.content_epoch == Some(epoch) {
            return;
        }
        if let (Some(shown), Some(new)) = (&self.shown, &value)
            && same_custom_value(shown, new)
        {
            self.title = title;
            self.content_epoch = Some(epoch);
            return;
        }
        self.set_content(title, value, epoch);
    }

    /// Shared present/refresh core: stage `value` for the next draw (or a
    /// "no image" message) and stamp the epoch. Framing is the caller's
    /// business.
    fn set_content(&mut self, title: String, value: Option<DynamicValue>, epoch: RunId) {
        self.title = title;
        self.content_epoch = Some(epoch);
        self.message = None;
        self.shown = value.clone();
        match value {
            Some(_) => self.pending = value,
            None => {
                self.pending = None;
                self.image = None;
                self.native_size = UVec2::ZERO;
                self.native_format = None;
                self.message = Some("port has no image value".to_owned());
            }
        }
    }

    /// Draw the viewer pane (the whole tab content). Converts + uploads a
    /// pending value first, then applies last frame's pan/zoom gestures,
    /// then paints image (or message) and the header readout.
    pub(crate) fn show(&mut self, ui: &mut Ui, theme: &Theme) {
        if let Some(value) = self.pending.take() {
            match render_full(&value) {
                Ok(rendered) => {
                    // Keep the user's framing across a same-size refresh
                    // (A/B-ing a parameter tweak); refit when dimensions
                    // changed — the old viewport frames nothing sensible.
                    let dims_changed = self
                        .image
                        .as_ref()
                        .is_none_or(|old| old.size() != rendered.image_size());
                    if dims_changed {
                        self.reset_framing();
                    }
                    self.native_size = rendered.native_size;
                    self.native_format = Some(rendered.native_format);
                    self.image = Some(ui.register_image(rendered.image));
                }
                Err(message) => {
                    self.image = None;
                    self.native_size = UVec2::ZERO;
                    self.native_format = None;
                    self.message = Some(message);
                }
            }
        }
        self.apply_gestures(ui);

        let pane = pane_size(ui, self.port);
        let fill = match self.background {
            ViewerBackground::Theme | ViewerBackground::Checker => theme.colors.canvas_bg,
            ViewerBackground::Black => Color::BLACK,
            ViewerBackground::White => Color::WHITE,
        };
        Panel::zstack()
            .id(pane_wid(self.port))
            .size((Sizing::FILL, Sizing::FILL))
            .sense(Sense::CLICK | Sense::DRAG | Sense::SCROLL | Sense::PINCH)
            .clip_rect()
            .background(Background {
                fill: fill.into(),
                ..Default::default()
            })
            .show(ui, |ui| {
                if self.background == ViewerBackground::Checker
                    && let Some(pane) = pane
                {
                    self.draw_checker(ui, pane);
                }
                match (&self.image, pane) {
                    (Some(handle), Some(pane)) => {
                        let v = self
                            .view
                            .unwrap_or_else(|| fit_viewport(handle.size().as_vec2(), pane));
                        ui.add_shape(Shape::Image {
                            handle: handle.clone(),
                            local_rect: Some(draw_rect(handle.size().as_vec2(), v)),
                            fit: ImageFit::Fill,
                            filter: self.filter,
                            tint: Color::WHITE,
                        });
                    }
                    // Pane not measured yet (first frame): let aperture fit it.
                    (Some(handle), None) => {
                        ui.add_shape(Shape::Image {
                            handle: handle.clone(),
                            local_rect: None,
                            fit: ImageFit::Contain,
                            filter: self.filter,
                            tint: Color::WHITE,
                        });
                    }
                    (None, _) => {
                        let hint = self
                            .message
                            .as_deref()
                            .unwrap_or("the port's image appears here after the next graph run");
                        Text::new(hint.to_owned())
                            .style(muted_style(theme, ui))
                            .align(Align::CENTER)
                            .show(ui);
                    }
                }
                self.header(ui, theme, pane);
                if self.image.is_some() {
                    self.controls(ui, theme, pane);
                }
            });
    }

    /// The screen-fixed checkerboard backdrop across the whole pane. One
    /// tiled 2×2 texture; `Nearest` keeps the squares crisp at any pane
    /// size and DPI.
    fn draw_checker(&mut self, ui: &mut Ui, pane: Vec2) {
        let handle = self
            .checker
            .get_or_insert_with(|| ui.register_image(checker_image()))
            .clone();
        ui.add_shape(Shape::Image {
            handle,
            local_rect: None,
            fit: ImageFit::Tile {
                offset: Vec2::ZERO,
                // The 2×2 tile is one checker period = 2 squares across.
                scale: pane / (2.0 * CHECKER_SQUARE_PX),
            },
            filter: ImageFilter::Nearest,
            tint: Color::WHITE,
        });
    }

    /// The top-left readout: source port, native dimensions, whether the
    /// view is texture-capped, and the current zoom.
    fn header(&self, ui: &mut Ui, theme: &Theme, pane: Option<Vec2>) {
        let Some(handle) = &self.image else {
            return;
        };
        let mut text = format!(
            "{} · {} × {}",
            if self.title.is_empty() {
                "image"
            } else {
                &self.title
            },
            self.native_size.x,
            self.native_size.y,
        );
        if let Some(format) = self.native_format {
            text.push_str(&format!(" · {format}"));
        }
        if handle.size() != self.native_size {
            text.push_str(" · downscaled view");
        }
        let zoom = match (self.view, pane) {
            (Some(v), _) => Some(v.zoom),
            (None, Some(pane)) => Some(fit_viewport(handle.size().as_vec2(), pane).zoom),
            (None, None) => None,
        };
        if let Some(zoom) = zoom {
            text.push_str(&format!(" · {:.0}%", zoom * 100.0));
        }
        // A frosted readout pill — the text sibling of the toolbar chrome,
        // floating inset from the corner like the control pills opposite,
        // so the readout stays legible over bright image regions.
        Panel::hstack()
            .id_salt("viewer_header")
            .size((Sizing::Hug, Sizing::Hug))
            .margin(Spacing::new(TOOLBAR_MARGIN, TOOLBAR_MARGIN, 0.0, 0.0))
            .padding(Spacing::new(10.0, 6.0, 10.0, 6.0))
            .background(pill_background(theme))
            .show(ui, |ui| {
                Text::new(text).style(muted_style(theme, ui)).show(ui);
            });
    }

    /// The floating control panel in the pane's top-right corner — the
    /// viewer twin of the graph toolbar: function groups on stacked
    /// frosted pills, opaque chip buttons raised off each pill. The top
    /// pill frames the view (fit, 100%); the column below holds
    /// appearance — the backdrop radio stack and, past a rule, the
    /// sampling toggle. Drawn after the image so the buttons hit-test
    /// above the pane's gesture surface. Framing clicks land next frame
    /// (responses lag the record by one frame) — imperceptible.
    fn controls(&mut self, ui: &mut Ui, theme: &Theme, pane: Option<Vec2>) {
        let port = self.port;
        Panel::vstack()
            .id(control_wid(port, "panel"))
            .size((Sizing::Hug, Sizing::Hug))
            .align(Align::new(HAlign::Right, VAlign::Top))
            .child_align(Align::new(HAlign::Right, VAlign::Top))
            .margin(Spacing::new(0.0, TOOLBAR_MARGIN, TOOLBAR_MARGIN, 0.0))
            .gap(BUTTON_GAP)
            .show(ui, |ui| {
                let framing = Panel::hstack().id(control_wid(port, "pill_framing"));
                pill(ui, theme, framing, |ui| {
                    if Chip::new(control_wid(port, "fit"), "Fit to view").show(ui, theme, draw_fit)
                    {
                        self.reset_framing();
                    }
                    if Chip::new(control_wid(port, "100"), "Zoom to 100%").show(ui, theme, draw_100)
                        && let (Some(handle), Some(pane)) = (&self.image, pane)
                    {
                        let img = handle.size().as_vec2();
                        let v = self.view.unwrap_or_else(|| fit_viewport(img, pane));
                        self.view = Some(zoom_about_pane_center(v, 1.0, pane));
                    }
                });
                let appearance = Panel::vstack().id(control_wid(port, "pill_appearance"));
                pill(ui, theme, appearance, |ui| {
                    for (mode, key, tip) in [
                        (ViewerBackground::Theme, "bg_theme", "Theme background"),
                        (ViewerBackground::Black, "bg_black", "Black background"),
                        (ViewerBackground::White, "bg_white", "White background"),
                        (
                            ViewerBackground::Checker,
                            "bg_checker",
                            "Checkerboard background",
                        ),
                    ] {
                        if self.background_swatch(ui, theme, mode, key, tip) {
                            self.background = mode;
                        }
                    }
                    // Rule between the backdrop radio stack and the
                    // sampling toggle — two concepts, one pill.
                    pill_rule(ui, theme);
                    self.filter_toggle(ui, theme);
                });
            });
    }

    /// One backdrop-mode button: a neutral chip whose glyph is a swatch
    /// of the mode itself, ringed with the selection accent when active.
    fn background_swatch(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        mode: ViewerBackground,
        key: &'static str,
        tip: &'static str,
    ) -> bool {
        let selected = self.background == mode;
        Chip::new(control_wid(self.port, key), tip).show(ui, theme, |ui, s, _| {
            draw_swatch(ui, s, theme, mode, selected)
        })
    }

    /// The nearest/bilinear sampling toggle: accent-filled while nearest
    /// is active, neutral otherwise.
    fn filter_toggle(&mut self, ui: &mut Ui, theme: &Theme) {
        let nearest = self.filter == ImageFilter::Nearest;
        let tip = if nearest {
            "Sampling: nearest — click for bilinear"
        } else {
            "Sampling: bilinear — click for nearest"
        };
        if Chip::new(control_wid(self.port, "filter"), tip)
            .toggled(nearest)
            .show(ui, theme, draw_pixels)
        {
            self.filter = if nearest {
                ImageFilter::Linear
            } else {
                ImageFilter::Nearest
            };
        }
    }

    /// Fold last frame's pane gestures into the viewport: left/middle-drag
    /// pans, wheel/pinch zooms about the cursor, two-finger scroll pans,
    /// double-click resets to fit. The fit viewport materializes into an
    /// explicit one on the first adjusting gesture.
    fn apply_gestures(&mut self, ui: &Ui) {
        let Some(handle) = &self.image else {
            return;
        };
        let resp = ui.response_for(pane_wid(self.port));
        let Some(pane) = pane_size(ui, self.port) else {
            return;
        };
        let img = handle.size().as_vec2();
        if img.x <= 0.0 || img.y <= 0.0 {
            return;
        }
        if resp.double_clicked() {
            self.reset_framing();
            return;
        }
        let adjusting = resp.drag_started_by(PointerButton::Left)
            || resp.drag_started_by(PointerButton::Middle)
            || resp.scroll_pixels != Vec2::ZERO
            || resp.scroll_lines.y.abs() > f32::EPSILON
            || (resp.zoom_factor - 1.0).abs() > f32::EPSILON;
        if self.view.is_none() && !adjusting {
            return;
        }
        let mut v = self.view.unwrap_or_else(|| fit_viewport(img, pane));

        if resp.drag_started_by(PointerButton::Left) || resp.drag_started_by(PointerButton::Middle)
        {
            self.pan_anchor = Some(v.pan);
        }
        let drag = resp
            .drag_delta_by(PointerButton::Left)
            .or_else(|| resp.drag_delta_by(PointerButton::Middle));
        match (self.pan_anchor, drag) {
            (Some(anchor), Some(d)) => v.pan = anchor + d,
            (Some(_), None) => self.pan_anchor = None,
            _ => {}
        }
        if resp.scroll_pixels != Vec2::ZERO {
            v.pan -= resp.scroll_pixels;
        }
        if resp.scroll_lines.y.abs() > f32::EPSILON
            && let Some(pivot) = resp.pointer_local
        {
            let line_px = ui.theme.text.line_height_for(ui.theme.text.font_size_px);
            zoom_about(
                &mut v.pan,
                &mut v.zoom,
                pivot,
                scroll_to_zoom_factor(resp.scroll_lines.y * line_px),
                MIN_ZOOM,
                MAX_ZOOM,
            );
        }
        if (resp.zoom_factor - 1.0).abs() > f32::EPSILON
            && let Some(pivot) = resp.pointer_local
        {
            zoom_about(
                &mut v.pan,
                &mut v.zoom,
                pivot,
                resp.zoom_factor,
                MIN_ZOOM,
                MAX_ZOOM,
            );
        }
        self.view = Some(v);
    }
}

impl RenderedImage {
    fn image_size(&self) -> UVec2 {
        UVec2::new(self.image.width, self.image.height)
    }
}

/// Display label for a viewer tab / pane header: the node's name (falling
/// back to "image" for an unnamed node) plus a compact port tag, so
/// several ports of one node stay tellable apart — e.g. "stack · out 1".
/// The one formatter for both the tab strip and the viewer title.
pub(crate) fn port_label(doc: &Document, port: PortRef) -> String {
    let name = doc
        .graph
        .find_node(&port.node_id, NodeSearch::Recursive)
        .map(|n| n.name.as_str())
        .filter(|n| !n.is_empty())
        .unwrap_or("image");
    let side = match port.kind {
        PortKind::Input => "in",
        PortKind::Output => "out",
    };
    format!("{name} \u{b7} {side} {}", port.port_idx)
}

/// Last frame's measured pane size, `None` before the first layout.
fn pane_size(ui: &Ui, port: PortRef) -> Option<Vec2> {
    let size = ui.response_for(pane_wid(port)).layout_rect?.size;
    (size.w > 0.0 && size.h > 0.0).then(|| Vec2::new(size.w, size.h))
}

/// Stable id for a viewer's pane — keyed by port so switching between two
/// viewer tabs can't cross-feed their gesture responses.
fn pane_wid(port: PortRef) -> WidgetId {
    WidgetId::from_hash(("image_viewer.pane", port))
}

/// Stable id for one control-panel widget, keyed by port + role.
fn control_wid(port: PortRef, key: &'static str) -> WidgetId {
    WidgetId::from_hash(("image_viewer.controls", port, key))
}

/// Checkerboard grays (sRGB bytes) — shared by the backdrop tile and
/// its control-panel swatch. Fixed regardless of theme: the checker is
/// a neutral transparency reference, not chrome.
const CHECKER_LIGHT_U8: u8 = 77; // #4d4d4d
const CHECKER_DARK_U8: u8 = 51; // #333333

/// The 2×2 checkerboard tile — one full checker period, stamped across
/// the pane via `ImageFit::Tile` + `ImageFilter::Nearest`.
fn checker_image() -> aperture::Image {
    const L: u8 = CHECKER_LIGHT_U8;
    const D: u8 = CHECKER_DARK_U8;
    let px = [
        [L, L, L, 255],
        [D, D, D, 255],
        [D, D, D, 255],
        [L, L, L, 255],
    ];
    aperture::Image::from_rgba8(2, 2, px.into_iter().flatten().collect())
}

/// The viewport at `zoom` that keeps the texel under the pane center
/// fixed — the button sibling of the cursor-anchored wheel zoom.
fn zoom_about_pane_center(mut v: ViewerViewport, zoom: f32, pane: Vec2) -> ViewerViewport {
    let factor = zoom / v.zoom;
    zoom_about(
        &mut v.pan,
        &mut v.zoom,
        pane * 0.5,
        factor,
        MIN_ZOOM,
        MAX_ZOOM,
    );
    v
}

/// Four inward corner brackets — "fit the image to the view".
fn draw_fit(ui: &mut Ui, s: f32, color: Color) {
    let t = s * 0.07; // bar thickness
    let len = s * 0.18; // bar length
    let o = s * 0.26; // inset from the button edge
    let far = s - o;
    // An L in each corner: horizontal bar + vertical bar.
    let bars = [
        (o, o, len, t),
        (o, o, t, len),
        (far - len, o, len, t),
        (far - t, o, t, len),
        (o, far - t, len, t),
        (o, far - len, t, len),
        (far - len, far - t, len, t),
        (far - t, far - len, t, len),
    ];
    for (x, y, w, h) in bars {
        ui.add_shape(Shape::RoundedRect {
            local_rect: Some(Rect::new(x, y, w, h)),
            corners: Corners::all(t * 0.5),
            fill: color.into(),
            stroke: Stroke::ZERO,
        });
    }
}

/// "1:1" label — zoom to 100%.
fn draw_100(ui: &mut Ui, _s: f32, color: Color) {
    let style = TextStyle {
        color,
        font_size_px: 11.0,
        ..ui.theme.text
    };
    Text::new("1:1").style(style).align(Align::CENTER).show(ui);
}

/// 2×2 grid of hard squares — nearest (pixelated) sampling.
fn draw_pixels(ui: &mut Ui, s: f32, color: Color) {
    let cell = s * 0.18;
    let gap = s * 0.08;
    let o = (s - (2.0 * cell + gap)) * 0.5;
    for iy in 0..2 {
        for ix in 0..2 {
            let x = o + ix as f32 * (cell + gap);
            let y = o + iy as f32 * (cell + gap);
            ui.add_shape(Shape::RoundedRect {
                local_rect: Some(Rect::new(x, y, cell, cell)),
                corners: Corners::all(1.0),
                fill: color.into(),
                stroke: Stroke::ZERO,
            });
        }
    }
}

/// A backdrop-mode swatch: an inset square filled with the mode itself
/// (mini checker for `Checker`), ringed with the selection accent when
/// active.
fn draw_swatch(ui: &mut Ui, s: f32, theme: &Theme, mode: ViewerBackground, selected: bool) {
    let d = s * 0.54;
    let o = (s - d) * 0.5;
    let rect = Rect::new(o, o, d, d);
    match mode {
        ViewerBackground::Checker => {
            let light = Color::rgb_u8(CHECKER_LIGHT_U8, CHECKER_LIGHT_U8, CHECKER_LIGHT_U8);
            let dark = Color::rgb_u8(CHECKER_DARK_U8, CHECKER_DARK_U8, CHECKER_DARK_U8);
            ui.add_shape(Shape::RoundedRect {
                local_rect: Some(rect),
                corners: Corners::all(2.0),
                fill: dark.into(),
                stroke: Stroke::ZERO,
            });
            // Two light quads on the diagonal make the 2×2 mini checker.
            let h = d * 0.5;
            for cell in [Rect::new(o, o, h, h), Rect::new(o + h, o + h, h, h)] {
                ui.add_shape(Shape::RoundedRect {
                    local_rect: Some(cell),
                    corners: Corners::all(0.0),
                    fill: light.into(),
                    stroke: Stroke::ZERO,
                });
            }
        }
        _ => {
            let fill = match mode {
                ViewerBackground::Theme => theme.colors.canvas_bg,
                ViewerBackground::Black => Color::BLACK,
                ViewerBackground::White => Color::WHITE,
                ViewerBackground::Checker => unreachable!(),
            };
            ui.add_shape(Shape::RoundedRect {
                local_rect: Some(rect),
                corners: Corners::all(2.0),
                fill: fill.into(),
                stroke: Stroke::ZERO,
            });
        }
    }
    // Ring on top so the checker quads can't cover it.
    let ring = if selected {
        Stroke::solid(theme.colors.selection_rect, 2.0)
    } else {
        Stroke::solid(theme.colors.text_muted.with_alpha(0.4), 1.0)
    };
    ui.add_shape(Shape::RoundedRect {
        local_rect: Some(rect),
        corners: Corners::all(2.0),
        fill: Color::TRANSPARENT.into(),
        stroke: ring,
    });
}

/// Whether two dynamic values share the same custom payload (`Arc`
/// identity) — a run that reuses a node's cache re-delivers the same
/// allocation, so the viewer can skip an identical re-render.
fn same_custom_value(a: &DynamicValue, b: &DynamicValue) -> bool {
    match (a, b) {
        (DynamicValue::Custom(a), DynamicValue::Custom(b)) => std::sync::Arc::ptr_eq(a, b),
        _ => false,
    }
}

/// The pane-local rect a viewport paints the texture into.
fn draw_rect(img: Vec2, v: ViewerViewport) -> Rect {
    Rect {
        min: v.pan,
        size: Size::new(img.x * v.zoom, img.y * v.zoom),
    }
}

/// Aspect-preserving fit of `img` (texture texels) centered in `pane`
/// (`ImageFit::Contain` semantics, upscaling small images too), as an
/// explicit viewport so the drawn fit and the gesture math can't drift.
fn fit_viewport(img: Vec2, pane: Vec2) -> ViewerViewport {
    let zoom = (pane.x / img.x).min(pane.y / img.y);
    ViewerViewport {
        pan: (pane - img * zoom) * 0.5,
        zoom,
    }
}

/// Longest-side cap of `native` to [`MAX_TEXTURE_DIM`], aspect-preserving,
/// never upscaling, at least 1×1.
fn capped_target(native: UVec2) -> UVec2 {
    let scale = (MAX_TEXTURE_DIM as f32 / native.x.max(native.y) as f32).min(1.0);
    UVec2::new(
        (native.x as f32 * scale).round().max(1.0) as u32,
        (native.y as f32 * scale).round().max(1.0) as u32,
    )
}

/// Convert a held image value to an uploadable RGBA8 raster: downcast to
/// the lens [`Image`](LensImage), read its CPU pixels (resident by
/// construction — the worker's thumbnail pass pulled the shared buffer to
/// the CPU; the `cpu_only` context is only a formality for `make_cpu`'s
/// signature), then cap + convert in one fused pass. `Err` carries the
/// user-facing reason.
fn render_full(value: &DynamicValue) -> Result<RenderedImage, String> {
    let image = value
        .as_custom::<LensImage>()
        .ok_or_else(|| "value is not an image".to_owned())?;
    let ctx = ProcessingContext::cpu_only();
    let cpu = image
        .buffer
        .make_cpu(&ctx)
        .map_err(|e| format!("could not read image pixels: {e}"))?;
    let native_size = UVec2::new(cpu.desc.width as u32, cpu.desc.height as u32);
    if native_size.x == 0 || native_size.y == 0 {
        return Err("image is empty".to_owned());
    }
    let native_format = cpu.desc.color_format;
    let target = capped_target(native_size);
    // 1:1 passes through as a plain RGBA8 convert (Preview never upscales).
    let rgba = Preview::new(target.x as usize, target.y as usize).to_rgba8(&cpu);
    let desc = rgba.desc;
    Ok(RenderedImage {
        image: aperture::Image::from_rgba8(
            desc.width as u32,
            desc.height as u32,
            rgba.into_bytes(),
        ),
        native_size,
        native_format,
    })
}

/// De-emphasized ink for the header/message, on the canvas fill.
fn muted_style(theme: &Theme, ui: &Ui) -> TextStyle {
    TextStyle {
        color: theme.colors.text_muted,
        font_size_px: 12.0,
        ..ui.theme.text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use imaginarium::{ColorFormat, Image as RawImage, ImageBuffer, ImageDesc};
    use scenarium::graph::NodeId;

    fn port() -> PortRef {
        PortRef {
            node_id: NodeId::from_u128(1),
            kind: PortKind::Output,
            port_idx: 0,
        }
    }

    fn image_value(width: usize, height: usize) -> DynamicValue {
        let desc = ImageDesc::new(width, height, ColorFormat::RGBA_U8);
        let raw = RawImage::new_with_data(desc, vec![128; width * height * 4]).unwrap();
        DynamicValue::from_custom(LensImage::new(ImageBuffer::from_cpu(raw)))
    }

    #[test]
    fn fit_viewport_centers_and_scales_like_contain() {
        // 400×200 texture in an 800×800 pane: width binds at zoom 2 —
        // Contain upscales. pan = ((800,800) - (800,400)) / 2 = (0, 200).
        let v = fit_viewport(Vec2::new(400.0, 200.0), Vec2::new(800.0, 800.0));
        assert_eq!(v.zoom, 2.0);
        assert_eq!(v.pan, Vec2::new(0.0, 200.0));

        // 4000×2000 in 1000×1000: zoom 0.25, pan = (0, (1000-500)/2) = (0, 250).
        let v = fit_viewport(Vec2::new(4000.0, 2000.0), Vec2::new(1000.0, 1000.0));
        assert_eq!(v.zoom, 0.25);
        assert_eq!(v.pan, Vec2::new(0.0, 250.0));

        // Height-bound case: 200×400 in 800×400 → zoom 1, pan = (300, 0).
        let v = fit_viewport(Vec2::new(200.0, 400.0), Vec2::new(800.0, 400.0));
        assert_eq!(v.zoom, 1.0);
        assert_eq!(v.pan, Vec2::new(300.0, 0.0));

        // The drawn rect covers exactly pan..pan+img*zoom.
        let r = draw_rect(Vec2::new(200.0, 400.0), v);
        assert_eq!(r.min, Vec2::new(300.0, 0.0));
        assert_eq!(r.size, Size::new(200.0, 400.0));
    }

    #[test]
    fn capped_target_shrinks_only_oversized_images() {
        // Under the cap: unchanged.
        assert_eq!(
            capped_target(UVec2::new(6000, 4000)),
            UVec2::new(6000, 4000)
        );
        // Exactly at the cap: unchanged.
        assert_eq!(capped_target(UVec2::new(8192, 100)), UVec2::new(8192, 100));
        // Over the cap: longest side pinned to 8192, aspect preserved
        // (16384×8192 → 8192×4096).
        assert_eq!(
            capped_target(UVec2::new(16384, 8192)),
            UVec2::new(8192, 4096)
        );
        // A degenerate-thin image never rounds to zero.
        assert_eq!(capped_target(UVec2::new(100_000, 1)), UVec2::new(8192, 1));
    }

    #[test]
    fn render_full_converts_a_cpu_image_and_rejects_non_images() {
        // A 2×1 RGBA_U8 image passes through byte-identical (1:1 target,
        // straight convert) and reports its native size.
        let desc = ImageDesc::new(2, 1, ColorFormat::RGBA_U8);
        let bytes = vec![255, 0, 0, 255, 0, 255, 0, 255];
        let raw = RawImage::new_with_data(desc, bytes.clone()).unwrap();
        let value = DynamicValue::from_custom(LensImage::new(ImageBuffer::from_cpu(raw)));

        let rendered = render_full(&value).expect("cpu image renders");
        assert_eq!(rendered.native_size, UVec2::new(2, 1));
        assert_eq!(rendered.native_format, ColorFormat::RGBA_U8);
        assert_eq!(rendered.image.width, 2);
        assert_eq!(rendered.image.height, 1);
        assert_eq!(rendered.image.pixels, bytes);

        // A non-RGBA8 source reports its own format — the header shows
        // the source pixels, not the RGBA8 view conversion. One RGB_F32
        // texel = 12 bytes.
        let desc = ImageDesc::new(1, 1, ColorFormat::RGB_F32);
        let raw = RawImage::new_with_data(desc, vec![0; 12]).unwrap();
        let value = DynamicValue::from_custom(LensImage::new(ImageBuffer::from_cpu(raw)));
        let rendered = render_full(&value).expect("f32 image renders");
        assert_eq!(rendered.native_format, ColorFormat::RGB_F32);
        assert_eq!(rendered.image.pixels.len(), 4, "converted to 1×1 RGBA8");

        // Non-image values are refused with a reason, not a panic.
        let err = render_full(&DynamicValue::from(42i64)).unwrap_err();
        assert!(err.contains("not an image"), "unexpected message: {err}");
    }

    #[test]
    fn refresh_updates_epoch_and_skips_identical_content() {
        let mut viewer = ImageViewer::new(port());
        let value = image_value(2, 2);

        // First refresh (restored tab, epoch never set): stages the value.
        viewer.refresh("stack · out 0".to_owned(), Some(value.clone()), 3);
        assert_eq!(viewer.content_epoch, Some(3));
        assert!(viewer.pending.is_some(), "first value stages a render");

        // Same epoch again: no-op even with a different value.
        viewer.pending = None;
        viewer.refresh("stack · out 0".to_owned(), Some(image_value(2, 2)), 3);
        assert!(viewer.pending.is_none(), "same epoch does not re-stage");

        // New epoch, same Arc (cache-reused node): adopts the epoch
        // without re-staging a render.
        viewer.refresh("stack · out 0".to_owned(), Some(value), 4);
        assert_eq!(viewer.content_epoch, Some(4));
        assert!(viewer.pending.is_none(), "identical Arc skips the render");

        // New epoch, new value: stages a render and keeps the framing.
        viewer.view = Some(ViewerViewport {
            pan: Vec2::new(5.0, 6.0),
            zoom: 2.0,
        });
        viewer.refresh("stack · out 0".to_owned(), Some(image_value(2, 2)), 5);
        assert_eq!(viewer.content_epoch, Some(5));
        assert!(viewer.pending.is_some(), "new value re-stages");
        assert!(viewer.view.is_some(), "refresh keeps the user's framing");
    }

    #[test]
    fn present_resets_framing_and_missing_value_stays_epochless() {
        let mut viewer = ImageViewer::new(port());
        viewer.view = Some(ViewerViewport {
            pan: Vec2::ZERO,
            zoom: 3.0,
        });

        // A click with a live value resets framing and stamps the epoch.
        viewer.present("n · out 0".to_owned(), Some(image_value(2, 2)), 7);
        assert!(viewer.view.is_none(), "present refits");
        assert_eq!(viewer.content_epoch, Some(7));

        // A click with no cached value shows a message but leaves the
        // epoch unset, so the next run's value still fills the pane.
        viewer.present("n · out 0".to_owned(), None, 7);
        assert_eq!(viewer.content_epoch, None);
        assert!(viewer.message.as_deref().unwrap().contains("next run"));
        // ...and a later refresh at any epoch takes.
        viewer.refresh("n · out 0".to_owned(), Some(image_value(2, 2)), 7);
        assert_eq!(viewer.content_epoch, Some(7));
        assert!(viewer.pending.is_some());
    }

    #[test]
    fn zoom_about_pane_center_keeps_center_texel() {
        // Start from the fit of 400×200 in an 800×800 pane: zoom 2,
        // pan (0, 200). The texel under the pane center (400, 400) is
        // ((400 - 0)/2, (400 - 200)/2) = (200, 100) — the image center.
        let fit = fit_viewport(Vec2::new(400.0, 200.0), Vec2::new(800.0, 800.0));
        assert_eq!(fit.zoom, 2.0);

        // Zoom to 100%: pan' = center - texel·1 = (400-200, 400-100).
        let pane = Vec2::new(800.0, 800.0);
        let v = zoom_about_pane_center(fit, 1.0, pane);
        assert_eq!(v.zoom, 1.0);
        assert_eq!(v.pan, Vec2::new(200.0, 300.0));

        // The invariant holds for an arbitrary target too: zoom 4 →
        // pan' = center - texel·4 = (400-800, 400-400) = (-400, 0).
        let v = zoom_about_pane_center(fit, 4.0, pane);
        assert_eq!(v.zoom, 4.0);
        assert_eq!(v.pan, Vec2::new(-400.0, 0.0));
    }

    #[test]
    fn checker_image_is_one_2x2_period() {
        let img = checker_image();
        assert_eq!((img.width, img.height), (2, 2));
        const L: u8 = CHECKER_LIGHT_U8;
        const D: u8 = CHECKER_DARK_U8;
        // Row-major light/dark, dark/light — one full checker period.
        #[rustfmt::skip]
        let expected = [
            L, L, L, 255,  D, D, D, 255,
            D, D, D, 255,  L, L, L, 255,
        ];
        assert_eq!(img.pixels, expected);
    }

    #[test]
    fn same_custom_value_is_arc_identity() {
        let a = image_value(1, 1);
        let clone = a.clone();
        let b = image_value(1, 1);
        assert!(same_custom_value(&a, &clone), "clone shares the Arc");
        assert!(!same_custom_value(&a, &b), "equal content, different Arc");
        assert!(!same_custom_value(&a, &DynamicValue::from(1i64)));
    }
}
