//! Full-resolution viewers for pinned ports' runtime images, one editor tab
//! per port ([`TabRef::ImageViewer`], deduped on open). Each visible viewer
//! reads its port's latest [`DynamicValue`] from the centralized [`RunState`]
//! and keeps only derived texture and navigation state. Opening or restoring
//! a tab therefore shows an already-received value without an editor-driven
//! notification path.
//!
//! The full RGBA8 conversion and texture upload happen lazily when a visible
//! viewer observes a new source revision. The buffer is CPU-resident by
//! construction: pinned-output receipt already prepared a thumbnail and
//! pulled it to the CPU in place (`ImageBuffer` interior mutability).
//!
//! [`RunState`]: crate::gui::run_state::RunState
//! [`TabRef::ImageViewer`]: crate::core::document::TabRef::ImageViewer

use std::fmt::Write as _;

use aperture::{
    Align, Background, Color, Configure, HAlign, Image as AptImage, ImageFilter, ImageFit,
    ImageHandle, InternedStr, Panel, Rect, Sense, Shape, Size, Sizing, Spacing, Text, Ui, VAlign,
    WidgetId,
};
use glam::{UVec2, Vec2};
use imaginarium::{ColorFormat, Image as RawImage, Preview, ProcessingContext};
use lens::Image as LensImage;
use scenarium::DynamicValue;
use scenarium::NodeSearch;

use crate::core::document::{Document, PortKind, PortRef, Viewport};
use crate::core::io::preferences::{ViewerBackground, ViewerPreferences};
use crate::gui::canvas::pan_zoom::{PanAnchor, fold_scroll_zoom, zoom_about};
use crate::gui::theme::Theme;
use crate::gui::widgets::support::{colored_text, filled_rect, muted_text, stroked_rect};
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

/// One image-viewer tab's state: what it shows and how it's framed.
/// Lives in the `MainWindow`'s per-port viewer map, keyed by (and
/// carrying) the [`PortRef`] its tab binds to; content is runtime-only
/// (never persisted).
#[derive(Debug)]
pub(crate) struct ImageViewer {
    /// The port this viewer shows — keys the pane's widget id so two
    /// viewer tabs never share gesture responses.
    port: PortRef,
    /// Revision of the centralized source reflected by `image` or `message`.
    source_revision: Option<u64>,
    /// The uploaded texture plus the source facts the header reports —
    /// present exactly while the pane shows an image.
    image: Option<ShownImage>,
    /// Why the pane is empty, when it is.
    message: Option<String>,
    /// Explicit viewport once the user pans/zooms; `None` = fit-to-pane
    /// (recomputed each frame, so it tracks pane resizes). The image's
    /// top-left offset in pane-local px plus the zoom (pane px per
    /// texture texel) — the same `local = pan + zoom * texel` mapping as
    /// the canvas, so the shared viewport algebra applies unchanged.
    view: Option<Viewport>,
    /// Pan-drag bookkeeping: the viewport pan at drag start.
    pan_anchor: PanAnchor,
    /// Lazily registered checkerboard tile for the `Checker` backdrop.
    /// The backdrop *choice* (and the sampling filter) live in
    /// [`ViewerPreferences`] — one persisted setting shared by every
    /// viewer tab, threaded into [`Self::show`] each frame.
    checker: Option<ImageHandle>,
}

/// The centralized source value a viewer borrows for one frame.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ImageSource<'a> {
    pub(crate) revision: u64,
    pub(crate) value: &'a DynamicValue,
}

/// A capped RGBA8 render of an image value, ready to register, plus the
/// source dimensions and pixel format it was derived from.
#[derive(Debug)]
pub(crate) struct RenderedImage {
    pub(crate) image: aperture::Image,
    pub(crate) native_size: UVec2,
    pub(crate) native_format: ColorFormat,
}

/// [`RenderedImage`] after upload: the texture handle plus the source
/// facts the header readout reports.
#[derive(Debug)]
struct ShownImage {
    handle: ImageHandle,
    /// Source dimensions before the texture-cap downscale.
    native_size: UVec2,
    /// Source pixel format before the RGBA8 view conversion.
    native_format: ColorFormat,
}

impl ImageViewer {
    /// An empty viewer for `port` (shows the hint until content arrives).
    pub(crate) fn new(port: PortRef) -> Self {
        Self {
            port,
            source_revision: None,
            image: None,
            message: None,
            view: None,
            pan_anchor: PanAnchor::default(),
            checker: None,
        }
    }

    /// Back to fit-to-pane framing (and cancel any pan in progress) for a
    /// source change, the fit button, or a double-click.
    fn reset_framing(&mut self) {
        self.view = None;
        self.pan_anchor.clear();
    }

    /// The framing to draw with: the user's explicit viewport, else the
    /// recomputed fit — the single source for the draw rect, the zoom
    /// readout, and the gesture/button math.
    fn effective_view(&self, img: Vec2, pane: Vec2) -> Viewport {
        self.view.unwrap_or_else(|| fit_viewport(img, pane))
    }

    /// Refresh derived content when the centralized source changes. The
    /// source itself is borrowed only for conversion and never retained by
    /// the view.
    fn sync_source(&mut self, source: Option<ImageSource<'_>>) -> Option<RenderedImage> {
        let revision = source.map(|source| source.revision);
        if revision == self.source_revision {
            return None;
        }
        self.source_revision = revision;
        self.message = None;
        match source {
            Some(source) => match render_full(source.value) {
                Ok(rendered) => Some(rendered),
                Err(message) => {
                    self.image = None;
                    self.message = Some(message);
                    None
                }
            },
            None => {
                self.image = None;
                self.message = Some("pinned output has no image value".to_owned());
                self.reset_framing();
                None
            }
        }
    }

    /// Draw the viewer pane (the whole tab content). Synchronizes from the
    /// borrowed source, uploads a newly derived raster, applies last frame's
    /// pan/zoom gestures, then paints the image (or message), header, and
    /// controls. Returns `true` when the shared viewer preferences changed.
    pub(crate) fn show(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        prefs: &mut ViewerPreferences,
        title: &str,
        source: Option<ImageSource<'_>>,
    ) -> bool {
        if let Some(rendered) = self.sync_source(source) {
            // Keep the user's framing across a same-size refresh (A/B-ing a
            // parameter tweak); refit when the dimensions changed.
            let dims_changed = self
                .image
                .as_ref()
                .is_none_or(|old| old.handle.size() != rendered.image_size());
            if dims_changed {
                self.reset_framing();
            }
            self.image = Some(ShownImage {
                native_size: rendered.native_size,
                native_format: rendered.native_format,
                handle: ui.register_image(rendered.image),
            });
        }
        self.apply_gestures(ui);

        let pane = pane_size(ui, self.port);
        let fill = match prefs.background {
            ViewerBackground::Theme | ViewerBackground::Checker => theme.colors.canvas_bg,
            ViewerBackground::Black => Color::BLACK,
            ViewerBackground::White => Color::WHITE,
        };
        let mut prefs_changed = false;
        Panel::zstack()
            .id(pane_wid(self.port))
            .size((Sizing::FILL, Sizing::FILL))
            .sense(Sense::CLICK | Sense::DRAG | Sense::SCROLL | Sense::PINCH)
            .clip_rect()
            .background(Background::fill(fill))
            .show(ui, |ui| {
                if prefs.background == ViewerBackground::Checker
                    && let Some(pane) = pane
                {
                    self.draw_checker(ui, pane);
                }
                match (&self.image, pane) {
                    (Some(shown), Some(pane)) => {
                        let img = shown.handle.size().as_vec2();
                        let v = self.effective_view(img, pane);
                        ui.add_shape(
                            Shape::image(shown.handle.clone())
                                .at(draw_rect(img, v))
                                .fit(ImageFit::Fill)
                                .filter(prefs.filter),
                        );
                    }
                    // Pane not measured yet (first frame): let aperture fit it.
                    (Some(shown), None) => {
                        ui.add_shape(
                            Shape::image(shown.handle.clone())
                                .fit(ImageFit::Contain)
                                .filter(prefs.filter),
                        );
                    }
                    (None, _) => {
                        let hint = self
                            .message
                            .as_deref()
                            .unwrap_or("the port's image appears here after the next graph run");
                        // On the frosted readout pill, so the hint stays
                        // legible over the checker/white backdrops too.
                        let text = ui.intern(hint);
                        readout_pill(
                            ui,
                            theme,
                            Panel::hstack().id_salt("viewer_hint").align(Align::CENTER),
                            text,
                        );
                    }
                }
                self.header(ui, theme, pane, title);
                if self.image.is_some() {
                    prefs_changed = self.controls(ui, theme, pane, prefs);
                }
            });
        prefs_changed
    }

    /// The screen-fixed checkerboard backdrop across the whole pane. One
    /// tiled 2×2 texture; `Nearest` keeps the squares crisp at any pane
    /// size and DPI.
    fn draw_checker(&mut self, ui: &mut Ui, pane: Vec2) {
        let handle = self
            .checker
            .get_or_insert_with(|| ui.register_image(checker_image()))
            .clone();
        ui.add_shape(
            Shape::image(handle)
                .fit(ImageFit::Tile {
                    offset: Vec2::ZERO,
                    // The 2×2 tile is one checker period = 2 squares across.
                    scale: pane / (2.0 * CHECKER_SQUARE_PX),
                })
                .filter(ImageFilter::Nearest),
        );
    }

    /// The top-left readout: source port, native dimensions and pixel
    /// format, whether the view is texture-capped, and the current zoom.
    /// (`title` is never empty — `port_label` supplies the fallback.)
    fn header(&self, ui: &mut Ui, theme: &Theme, pane: Option<Vec2>, title: &str) {
        let Some(shown) = &self.image else {
            return;
        };
        let mut text = format!(
            "{} · {} × {} · {}",
            title, shown.native_size.x, shown.native_size.y, shown.native_format,
        );
        if shown.handle.size() != shown.native_size {
            text.push_str(" · downscaled view");
        }
        let img = shown.handle.size().as_vec2();
        let zoom = match (self.view, pane) {
            (Some(v), _) => Some(v.zoom),
            (None, Some(pane)) => Some(self.effective_view(img, pane).zoom),
            // Pane not measured yet (first frame): no fit zoom to report.
            (None, None) => None,
        };
        if let Some(zoom) = zoom {
            let _ = write!(text, " · {:.0}%", zoom * 100.0);
        }
        readout_pill(
            ui,
            theme,
            Panel::hstack()
                .id_salt("viewer_header")
                .margin(Spacing::new(TOOLBAR_MARGIN, TOOLBAR_MARGIN, 0.0, 0.0)),
            text,
        );
    }

    /// The floating control panel in the pane's top-right corner — the
    /// viewer twin of the graph toolbar: function groups on stacked
    /// frosted pills, opaque chip buttons raised off each pill. The top
    /// pill frames the view (fit, 100%); the column below edits the
    /// shared appearance preferences — the backdrop radio stack and,
    /// past a rule, the sampling toggle. Returns `true` when `prefs`
    /// changed. Drawn after the image so the buttons hit-test above the
    /// pane's gesture surface. Framing clicks land next frame (responses
    /// lag the record by one frame) — imperceptible.
    fn controls(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        pane: Option<Vec2>,
        prefs: &mut ViewerPreferences,
    ) -> bool {
        let port = self.port;
        let mut changed = false;
        Panel::vstack()
            .id(control_wid(port, "panel"))
            .size((Sizing::HUG, Sizing::HUG))
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
                        && let (Some(shown), Some(pane)) = (&self.image, pane)
                    {
                        let img = shown.handle.size().as_vec2();
                        let v = self.effective_view(img, pane);
                        self.view = Some(zoom_about_pane_center(v, 1.0, pane));
                    }
                });
                let appearance = Panel::vstack().id(control_wid(port, "pill_appearance"));
                pill(ui, theme, appearance, |ui| {
                    for (mode, key, tip) in BACKDROPS {
                        let selected = prefs.background == mode;
                        if background_swatch(ui, theme, port, mode, selected, key, tip) && !selected
                        {
                            prefs.background = mode;
                            changed = true;
                        }
                    }
                    // Rule between the backdrop radio stack and the
                    // sampling toggle — two concepts, one pill.
                    pill_rule(ui, theme);
                    changed |= filter_toggle(ui, theme, port, &mut prefs.filter);
                });
            });
        changed
    }

    /// Fold last frame's pane gestures into the viewport: left/middle-drag
    /// pans, wheel/pinch zooms about the cursor, two-finger scroll pans,
    /// double-click resets to fit. The fit viewport materializes into an
    /// explicit one on the first adjusting gesture.
    fn apply_gestures(&mut self, ui: &Ui) {
        let Some(shown) = &self.image else {
            return;
        };
        // Registered images have non-zero dims by construction, so the
        // texel size is always a valid divisor.
        let img = shown.handle.size().as_vec2();
        let resp = ui.response_for(pane_wid(self.port));
        let Some(pane) = pane_size(ui, self.port) else {
            return;
        };
        if resp.left.double_clicked() {
            self.reset_framing();
            return;
        }
        let adjusting = resp.left.drag.started()
            || resp.middle.drag.started()
            || resp.scroll.pixels != Vec2::ZERO
            || resp.scroll.lines.y.abs() > f32::EPSILON
            || (resp.scroll.zoom - 1.0).abs() > f32::EPSILON;
        if self.view.is_none() && !adjusting {
            return;
        }
        let mut v = self.effective_view(img, pane);

        if resp.left.drag.started() || resp.middle.drag.started() {
            self.pan_anchor.latch(v.pan);
        }
        let drag = resp.left.drag.delta().or_else(|| resp.middle.drag.delta());
        self.pan_anchor.apply(drag, &mut v.pan);
        fold_scroll_zoom(&mut v, ui, &resp, MIN_ZOOM, MAX_ZOOM);
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

/// The backdrop radio roster — mode, widget-id key, tooltip — the one
/// table behind the controls loop and the swatch ids.
const BACKDROPS: [(ViewerBackground, &str, &str); 4] = [
    (ViewerBackground::Theme, "bg_theme", "Theme background"),
    (ViewerBackground::Black, "bg_black", "Black background"),
    (ViewerBackground::White, "bg_white", "White background"),
    (
        ViewerBackground::Checker,
        "bg_checker",
        "Checkerboard background",
    ),
];

/// One backdrop-mode chip: its glyph is a swatch of the mode itself,
/// ringed with the selection accent when `selected`. Returns whether it
/// was clicked.
fn background_swatch(
    ui: &mut Ui,
    theme: &Theme,
    port: PortRef,
    mode: ViewerBackground,
    selected: bool,
    key: &'static str,
    tip: &'static str,
) -> bool {
    Chip::new(control_wid(port, key), tip).show(ui, theme, |ui, s, _| {
        draw_swatch(ui, s, theme, mode, selected)
    })
}

/// A frosted readout chip — the text sibling of the toolbar pills —
/// dressing `panel` (caller-configured id + placement) in the shared
/// chrome around one line of muted text. Used by the header readout and
/// the empty-pane hint.
fn readout_pill(ui: &mut Ui, theme: &Theme, panel: Panel, text: impl Into<InternedStr>) {
    let text = text.into();
    panel
        .size((Sizing::HUG, Sizing::HUG))
        .padding(Spacing::new(10.0, 6.0, 10.0, 6.0))
        .background(pill_background(theme))
        .show(ui, |ui| {
            let style = muted_text(ui, theme, 12.0);
            Text::new(text).style(style).show(ui);
        });
}

/// The nearest/bilinear sampling toggle: accent-filled while nearest is
/// active. Flips `filter` on click; returns whether it changed.
fn filter_toggle(ui: &mut Ui, theme: &Theme, port: PortRef, filter: &mut ImageFilter) -> bool {
    let nearest = *filter == ImageFilter::Nearest;
    let tip = if nearest {
        "Sampling: nearest — click for bilinear"
    } else {
        "Sampling: bilinear — click for nearest"
    };
    if Chip::new(control_wid(port, "filter"), tip)
        .toggled(nearest)
        .show(ui, theme, draw_pixels)
    {
        *filter = if nearest {
            ImageFilter::Linear
        } else {
            ImageFilter::Nearest
        };
        return true;
    }
    false
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
fn zoom_about_pane_center(mut v: Viewport, zoom: f32, pane: Vec2) -> Viewport {
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
        filled_rect(ui, Rect::new(x, y, w, h), t * 0.5, color);
    }
}

/// "1:1" label — zoom to 100%.
fn draw_100(ui: &mut Ui, _s: f32, color: Color) {
    let style = colored_text(ui, color, 11.0);
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
            filled_rect(ui, Rect::new(x, y, cell, cell), 1.0, color);
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
            filled_rect(ui, rect, 2.0, dark);
            // Two light quads on the diagonal make the 2×2 mini checker.
            let h = d * 0.5;
            for cell in [Rect::new(o, o, h, h), Rect::new(o + h, o + h, h, h)] {
                filled_rect(ui, cell, 0.0, light);
            }
        }
        _ => {
            let fill = match mode {
                ViewerBackground::Theme => theme.colors.canvas_bg,
                ViewerBackground::Black => Color::BLACK,
                ViewerBackground::White => Color::WHITE,
                ViewerBackground::Checker => unreachable!(),
            };
            filled_rect(ui, rect, 2.0, fill);
        }
    }
    // Ring on top so the checker quads can't cover it.
    let (ring, width) = if selected {
        (theme.colors.selection_rect, 2.0)
    } else {
        (theme.colors.text_muted.with_alpha(0.4), 1.0)
    };
    stroked_rect(ui, rect, 2.0, ring, width);
}

/// The pane-local rect a viewport paints the texture into.
fn draw_rect(img: Vec2, v: Viewport) -> Rect {
    Rect {
        min: v.pan,
        size: Size::new(img.x * v.zoom, img.y * v.zoom),
    }
}

/// Aspect-preserving fit of `img` (texture texels) centered in `pane`
/// (`ImageFit::Contain` semantics, upscaling small images too), as an
/// explicit viewport so the drawn fit and the gesture math can't drift.
fn fit_viewport(img: Vec2, pane: Vec2) -> Viewport {
    let zoom = (pane.x / img.x).min(pane.y / img.y);
    Viewport {
        pan: (pane - img * zoom) * 0.5,
        zoom,
    }
}

/// Longest-side cap of `native` to `max_dim`, aspect-preserving, never
/// upscaling, at least 1×1.
fn capped_target(native: UVec2, max_dim: u32) -> UVec2 {
    let scale = (max_dim as f32 / native.x.max(native.y) as f32).min(1.0);
    UVec2::new(
        (native.x as f32 * scale).round().max(1.0) as u32,
        (native.y as f32 * scale).round().max(1.0) as u32,
    )
}

/// Convert a held image value to an uploadable RGBA8 raster capped to
/// `max_dim` on its longest side, plus the source's native size/format
/// before that cap: downcast to the lens [`Image`](LensImage), read its CPU
/// pixels (resident by construction — pinned-output preview preparation pulls
/// the shared buffer to the CPU; the `cpu_only` context is only a formality for
/// `make_cpu`'s signature), then cap + convert in one fused pass. `Err`
/// carries the user-facing reason. Shared by the viewer's full-resolution
/// render (capped to [`MAX_TEXTURE_DIM`]) and the canvas pin-preview
/// thumbnail (capped much smaller).
pub(crate) fn convert_image_value(
    value: &DynamicValue,
    max_dim: u32,
) -> Result<RenderedImage, String> {
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
    let target = capped_target(native_size, max_dim);
    // 1:1 passes through as a plain RGBA8 convert (Preview never upscales).
    let rgba = Preview::new(target.x as usize, target.y as usize).to_rgba8(&cpu);
    let image = rgba8_image(rgba).expect("Preview::to_rgba8 yields packed RGBA_U8");
    Ok(RenderedImage {
        image,
        native_size,
        native_format,
    })
}

/// Convert a held image value to an uploadable RGBA8 raster capped to
/// [`MAX_TEXTURE_DIM`], plus the source dimensions and pixel format it was
/// derived from.
fn render_full(value: &DynamicValue) -> Result<RenderedImage, String> {
    convert_image_value(value, MAX_TEXTURE_DIM as u32)
}

/// Reinterpret a packed `RGBA_U8` imaginarium image as an uploadable
/// aperture image. `None` for another format or a padded stride;
/// imaginarium images are tightly packed, so for RGBA_U8 input `Some` is
/// the norm and the bytes move without a repack.
fn rgba8_image(image: RawImage) -> Option<AptImage> {
    let desc = image.desc;
    if desc.color_format != ColorFormat::RGBA_U8 {
        return None;
    }
    let pixels = image.into_bytes();
    if pixels.len() != desc.row_bytes() * desc.height {
        return None;
    }
    Some(AptImage::from_rgba8(
        desc.width as u32,
        desc.height as u32,
        pixels,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use imaginarium::{ColorFormat, Image as RawImage, ImageBuffer, ImageDesc};
    use scenarium::NodeId;

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
        let cap = MAX_TEXTURE_DIM as u32;
        // Under the cap: unchanged.
        assert_eq!(
            capped_target(UVec2::new(6000, 4000), cap),
            UVec2::new(6000, 4000)
        );
        // Exactly at the cap: unchanged.
        assert_eq!(
            capped_target(UVec2::new(8192, 100), cap),
            UVec2::new(8192, 100)
        );
        // Over the cap: longest side pinned to 8192, aspect preserved
        // (16384×8192 → 8192×4096).
        assert_eq!(
            capped_target(UVec2::new(16384, 8192), cap),
            UVec2::new(8192, 4096)
        );
        // A degenerate-thin image never rounds to zero.
        assert_eq!(
            capped_target(UVec2::new(100_000, 1), cap),
            UVec2::new(8192, 1)
        );
        // A smaller cap (the pin-preview thumbnail's use case) scales down
        // the same way, independent of `MAX_TEXTURE_DIM`.
        assert_eq!(
            capped_target(UVec2::new(1024, 512), 256),
            UVec2::new(256, 128)
        );
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
    fn sync_source_tracks_revisions_without_retaining_values() {
        let mut viewer = ImageViewer::new(port());
        viewer.view = Some(Viewport {
            pan: Vec2::ZERO,
            zoom: 3.0,
        });
        let first = image_value(2, 2);

        let rendered = viewer
            .sync_source(Some(ImageSource {
                revision: 1,
                value: &first,
            }))
            .expect("a fresh source is rendered");
        assert_eq!(rendered.image_size(), UVec2::new(2, 2));
        assert_eq!(viewer.source_revision, Some(1));
        assert_eq!(
            viewer.view,
            Some(Viewport {
                pan: Vec2::ZERO,
                zoom: 3.0,
            }),
            "conversion defers any refit until the raster is uploaded"
        );

        let same_revision = image_value(3, 1);
        let rendered = viewer.sync_source(Some(ImageSource {
            revision: 1,
            value: &same_revision,
        }));
        assert!(
            rendered.is_none(),
            "an unchanged revision is not reconverted"
        );

        let rendered = viewer
            .sync_source(Some(ImageSource {
                revision: 2,
                value: &same_revision,
            }))
            .expect("a newer revision is converted");
        assert_eq!(rendered.image_size(), UVec2::new(3, 1));

        assert!(viewer.sync_source(None).is_none());
        assert_eq!(viewer.source_revision, None);
        assert!(viewer.image.is_none());
        assert!(viewer.view.is_none(), "removing the source clears framing");
        assert_eq!(
            viewer.message.as_deref(),
            Some("pinned output has no image value")
        );
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
}
