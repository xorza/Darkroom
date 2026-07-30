//! Full-resolution viewers for preview nodes' runtime images, one editor tab
//! per node ([`TabRef::ImageViewer`], deduped on open). Each visible viewer
//! borrows its node's registered texture from the centralized preview
//! store and keeps only navigation state. Opening or restoring a tab therefore
//! shows an already-received value without an editor-driven notification path.
//!
//! The store materializes the full RGBA8 texture before the viewer records and
//! releases the source value immediately after registration. It does that only
//! for the nodes a pane will actually show
//! (`Document::visible_viewer_nodes`), so a viewer stacked behind another tab
//! holds no full-resolution texture until it is activated.
//!
//! [`TabRef::ImageViewer`]: crate::core::document::TabRef::ImageViewer

use scenarium::NodeId;
use std::fmt::Write as _;

use glam::{UVec2, Vec2};
use imaginarium::ColorFormat;
use palantir::{
    Align, Background, Color, Configure, HAlign, ImageFilter, ImageFit, ImageHandle, Panel, Rect,
    Sense, Shape, Size, Sizing, Spacing, Text, TextInput, Ui, VAlign, WidgetId,
};

use crate::core::document::{Document, Viewport};
use crate::core::io::preferences::{ViewerBackground, ViewerPreferences};
use crate::gui::canvas::pan_zoom::{fold_scroll_zoom, zoom_about};
use crate::gui::preview_store::{FullImage, StoredContent};
use crate::gui::theme::Theme;
use crate::gui::widgets::support::{colored_text, filled_rect, muted_text, stroked_rect};
use crate::gui::widgets::toolbar::{
    BUTTON_GAP, Chip, TOOLBAR_MARGIN, pill, pill_background, pill_rule,
};

/// Viewer zoom bounds — far wider than the canvas's
/// (`pan_zoom::CANVAS_MIN_ZOOM`/`CANVAS_MAX_ZOOM`): out to overview a
/// texture-capped 8k frame in a small pane, in for pixel peeping. Named apart
/// from the canvas pair because both are passed into the same shared
/// `fold_scroll_zoom` / `zoom_about`, where an unqualified `MIN_ZOOM` at the
/// call site wouldn't say which surface's range is in play.
const VIEWER_MIN_ZOOM: f32 = 0.02;
const VIEWER_MAX_ZOOM: f32 = 32.0;

/// On-screen side of one checkerboard square, logical px. Screen-fixed
/// (doesn't pan/zoom with the image) — it's a transparency reference,
/// not content.
const CHECKER_SQUARE_PX: f32 = 8.0;

/// One image-viewer tab's state: what it shows and how it's framed.
/// Lives in the `MainWindow`'s per-node viewer map, keyed by (and
/// carrying) the [`NodeId`] its tab binds to; content is runtime-only
/// (never persisted).
#[derive(Debug)]
pub(crate) struct ImageViewer {
    /// The preview node this viewer shows — keys the pane's widget id so
    /// two viewer tabs never share gesture responses.
    node_id: NodeId,
    /// Texture dimensions used to decide whether a new revision needs a refit.
    source_size: Option<UVec2>,
    /// Explicit viewport once the user pans/zooms; `None` = fit-to-pane
    /// (recomputed each frame, so it tracks pane resizes). The image's
    /// top-left offset in pane-local logical px plus the zoom (physical
    /// display px per texture texel). Texture dimensions are converted to
    /// their 1:1 logical footprint before applying it.
    view: Option<Viewport>,
    /// Pan-drag bookkeeping: the viewport pan at drag start. A bare
    /// `Option` — one viewer is one surface, so there is no pane to key
    /// it by the way the canvas has to.
    pan_anchor: Option<Vec2>,
    /// Lazily registered checkerboard tile for the `Checker` backdrop.
    /// The backdrop choice and magnification filter live in
    /// [`ViewerPreferences`] — one persisted setting shared by every
    /// viewer tab, threaded into [`Self::show`] each frame.
    checker: Option<ImageHandle>,
}

#[derive(Clone, Copy, Debug)]
struct ShownImage<'a> {
    handle: &'a ImageHandle,
    /// Source dimensions before the texture-cap downscale.
    native_size: UVec2,
    /// Source pixel format before the RGBA8 view conversion.
    native_format: ColorFormat,
}

impl ImageViewer {
    /// An empty viewer for `node_id` (shows the hint until content arrives).
    pub(crate) fn new(node_id: NodeId) -> Self {
        Self {
            node_id,
            source_size: None,
            view: None,
            pan_anchor: None,
            checker: None,
        }
    }

    /// Back to fit-to-pane framing (and cancel any pan in progress) for a
    /// source change, the fit button, or a double-click.
    fn reset_framing(&mut self) {
        self.view = None;
        self.pan_anchor = None;
    }

    /// The framing to draw with: the user's explicit viewport, else the
    /// recomputed fit — the single source for the draw rect, the zoom
    /// readout, and the gesture/button math.
    fn effective_view(&self, img: Vec2, pane: Vec2) -> Viewport {
        self.view.unwrap_or_else(|| fit_viewport(img, pane))
    }

    /// Keep framing across same-size revisions and refit when the displayed
    /// texture dimensions change or the source disappears.
    fn sync_source(&mut self, source_size: Option<UVec2>) {
        if source_size != self.source_size {
            self.reset_framing();
            self.source_size = source_size;
        }
    }

    /// Draw the viewer pane (the whole tab content). Borrows the centralized
    /// texture, applies last frame's pan/zoom gestures, then paints the image
    /// (or message), header, and controls. Returns `true` when the shared
    /// viewer preferences changed.
    pub(crate) fn show(
        &mut self,
        ui: &mut Ui,
        theme: &Theme,
        prefs: &mut ViewerPreferences,
        title: &str,
        source: Option<&StoredContent>,
    ) -> bool {
        let (shown, message) = match source {
            // A stored image still has to clear the viewer's own bar: only the
            // full-resolution upload is worth showing here, where the pin
            // card is happy with the thumbnail.
            Some(value) => match value.image() {
                Some(image) => match &image.full {
                    FullImage::Resident(handle) => (
                        Some(ShownImage {
                            handle,
                            native_size: image.native_size,
                            native_format: image.native_format,
                        }),
                        None,
                    ),
                    FullImage::Failed(message) => (None, Some(message.as_str())),
                    FullImage::Deferred(_) => {
                        // This pane is its group's visible tab, and the
                        // reconcile pass runs every frame ahead of the record,
                        // so it covered this viewer — unless the tab became
                        // visible after that pass, within this same frame.
                        debug_assert!(
                            false,
                            "visible image viewer source was not materialized: \
                             the frame's reconcile pass did not cover it"
                        );
                        (None, Some("image is being prepared"))
                    }
                },
                // No image to show. A failure reports its own reason; a
                // perfectly good non-image value doesn't get one, because
                // "7" is not what a viewer tab is for — the preview card
                // renders that.
                None => (
                    None,
                    Some(match value {
                        StoredContent::Error(message) => message.as_str(),
                        _ => "this preview has no image value",
                    }),
                ),
            },
            None => (None, None),
        };
        self.sync_source(shown.map(|image| image.handle.size()));
        self.apply_gestures(ui, shown);

        let pane = pane_size(ui, self.node_id);
        let display_scale = ui.display().scale_factor;
        let fill = match prefs.background {
            ViewerBackground::Theme | ViewerBackground::Checker => theme.colors.canvas_bg,
            ViewerBackground::Black => Color::BLACK,
            ViewerBackground::White => Color::WHITE,
        };
        let mut prefs_changed = false;
        Panel::zstack()
            .id(pane_wid(self.node_id))
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
                match (shown, pane) {
                    (Some(shown), Some(pane)) => {
                        let img = logical_image_size(shown.handle.size(), display_scale);
                        let v = self.effective_view(img, pane);
                        ui.add_shape(
                            Shape::image(shown.handle.clone())
                                .at(draw_rect(img, v))
                                .fit(ImageFit::Fill)
                                .min_filter(ImageFilter::Linear)
                                .mag_filter(prefs.mag_filter),
                        );
                    }
                    // Pane not measured yet (first frame): let palantir fit it.
                    (Some(shown), None) => {
                        ui.add_shape(
                            Shape::image(shown.handle.clone())
                                .fit(ImageFit::Contain)
                                .min_filter(ImageFilter::Linear)
                                .mag_filter(prefs.mag_filter),
                        );
                    }
                    (None, _) => {
                        let hint = message
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
                if let Some(shown) = shown {
                    self.header(ui, theme, pane, title, shown);
                    prefs_changed = self.controls(ui, theme, pane, prefs, shown);
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
            .get_or_insert_with(|| {
                ui.register_image(checker_image())
                    .expect("checker image fits every supported GPU")
            })
            .clone();
        ui.add_shape(
            Shape::image(handle)
                .fit(ImageFit::Tile {
                    offset: Vec2::ZERO,
                    // The 2×2 tile is one checker period = 2 squares across.
                    scale: pane / (2.0 * CHECKER_SQUARE_PX),
                })
                .min_filter(ImageFilter::Nearest)
                .mag_filter(ImageFilter::Nearest),
        );
    }

    /// The top-left readout: source node, native dimensions and pixel
    /// format, whether the view is texture-capped, and the current zoom.
    /// (`title` is never empty — [`node_label`] supplies the fallback.)
    fn header(
        &self,
        ui: &mut Ui,
        theme: &Theme,
        pane: Option<Vec2>,
        title: &str,
        shown: ShownImage<'_>,
    ) {
        let mut text = format!(
            "{} · {} × {} · {}",
            title, shown.native_size.x, shown.native_size.y, shown.native_format,
        );
        if shown.handle.size() != shown.native_size {
            text.push_str(" · downscaled view");
        }
        let img = logical_image_size(shown.handle.size(), ui.display().scale_factor);
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
        shown: ShownImage<'_>,
    ) -> bool {
        let node_id = self.node_id;
        let mut changed = false;
        Panel::vstack()
            .id(control_wid(node_id, "panel"))
            .size((Sizing::HUG, Sizing::HUG))
            .align(Align::new(HAlign::Right, VAlign::Top))
            .child_align(Align::new(HAlign::Right, VAlign::Top))
            .margin(Spacing::new(0.0, TOOLBAR_MARGIN, TOOLBAR_MARGIN, 0.0))
            .gap(BUTTON_GAP)
            .show(ui, |ui| {
                let framing = Panel::hstack().id(control_wid(node_id, "pill_framing"));
                pill(ui, theme, framing, |ui| {
                    if Chip::new(control_wid(node_id, "fit"), "Fit to view")
                        .show(ui, theme, draw_fit)
                    {
                        self.reset_framing();
                    }
                    if Chip::new(control_wid(node_id, "100"), "Zoom to 100%")
                        .show(ui, theme, draw_100)
                        && let Some(pane) = pane
                    {
                        let img =
                            logical_image_size(shown.handle.size(), ui.display().scale_factor);
                        let v = self.effective_view(img, pane);
                        self.view = Some(zoom_about_pane_center(v, 1.0, pane));
                    }
                });
                let appearance = Panel::vstack().id(control_wid(node_id, "pill_appearance"));
                pill(ui, theme, appearance, |ui| {
                    for (mode, key, tip) in BACKDROPS {
                        let selected = prefs.background == mode;
                        if Chip::new(control_wid(node_id, key), tip).show(ui, theme, |ui, s, _| {
                            draw_swatch(ui, s, theme, mode, selected)
                        }) && !selected
                        {
                            prefs.background = mode;
                            changed = true;
                        }
                    }
                    // Rule between the backdrop radio stack and the
                    // sampling toggle — two concepts, one pill.
                    pill_rule(ui, theme);
                    changed |= filter_toggle(ui, theme, node_id, &mut prefs.mag_filter);
                });
            });
        changed
    }

    /// Fold last frame's pane gestures into the viewport: left/middle-drag
    /// pans, wheel/pinch zooms about the cursor, two-finger scroll pans,
    /// double-click resets to fit. The fit viewport materializes into an
    /// explicit one on the first adjusting gesture.
    fn apply_gestures(&mut self, ui: &Ui, shown: Option<ShownImage<'_>>) {
        let Some(shown) = shown else {
            return;
        };
        // Registered images have non-zero dims by construction, so the
        // texel size is always a valid divisor.
        let img = logical_image_size(shown.handle.size(), ui.display().scale_factor);
        let resp = ui.response_for(pane_wid(self.node_id));
        let Some(pane) = pane_size(ui, self.node_id) else {
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
            self.pan_anchor = Some(v.pan);
        }
        let drag = resp.left.drag.delta().or_else(|| resp.middle.drag.delta());
        // Measured from the latch, not integrated per frame, so a pan
        // lands where the pointer says however many frames it took; a
        // missing delta after a latch is the release edge.
        if let Some(start) = self.pan_anchor {
            match drag {
                Some(d) => v.pan = start + d,
                None => self.pan_anchor = None,
            }
        }
        fold_scroll_zoom(&mut v, ui, &resp, VIEWER_MIN_ZOOM, VIEWER_MAX_ZOOM);
        self.view = Some(v);
    }
}

/// Display label for a viewer tab / pane header: the node's name (falling
/// back to "image" for an unnamed node) plus a compact port tag, so
/// several ports of one node stay tellable apart — e.g. "stack · out 1".
/// The one formatter for both the tab strip and the viewer title.
///
/// A recursive whole-document node search plus a fresh `String`, so
/// resolve it once per tab per frame rather than once per reader.
pub(crate) fn node_label(doc: &Document, node_id: NodeId) -> String {
    doc.graph
        .find(node_id)
        .map(|n| n.name.as_str())
        .filter(|n| !n.is_empty())
        .unwrap_or("image")
        .to_owned()
}

/// Last frame's measured pane size, `None` before the first layout.
fn pane_size(ui: &Ui, node_id: NodeId) -> Option<Vec2> {
    let size = ui.response_for(pane_wid(node_id)).layout_rect?.size;
    (size.w > 0.0 && size.h > 0.0).then(|| Vec2::new(size.w, size.h))
}

/// Stable id for a viewer's pane — keyed by node so switching between two
/// viewer tabs can't cross-feed their gesture responses.
fn pane_wid(node_id: NodeId) -> WidgetId {
    WidgetId::from_hash(("image_viewer.pane", node_id))
}

/// Stable id for one control-panel widget, keyed by node + role.
fn control_wid(node_id: NodeId, key: &'static str) -> WidgetId {
    WidgetId::from_hash(("image_viewer.controls", node_id, key))
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

/// A frosted readout chip — the text sibling of the toolbar pills —
/// dressing `panel` (caller-configured id + placement) in the shared
/// chrome around one line of muted text. Used by the header readout and
/// the empty-pane hint.
fn readout_pill<'a>(ui: &mut Ui, theme: &Theme, panel: Panel, text: impl Into<TextInput<'a>>) {
    let text = text.into();
    panel
        .size((Sizing::HUG, Sizing::HUG))
        .padding(Spacing::new(10.0, 6.0, 10.0, 6.0))
        .background(pill_background(theme))
        .show(ui, |ui| {
            let style = muted_text(ui, theme, 12.0);
            Text::new(text).style(&style).show(ui);
        });
}

/// The nearest/bilinear magnification toggle: accent-filled while nearest
/// is active. Flips `filter` on click; returns whether it changed.
fn filter_toggle(ui: &mut Ui, theme: &Theme, node_id: NodeId, filter: &mut ImageFilter) -> bool {
    let nearest = *filter == ImageFilter::Nearest;
    let tip = if nearest {
        "Zoom-in sampling: nearest — click for bilinear"
    } else {
        "Zoom-in sampling: bilinear — click for nearest"
    };
    if Chip::new(control_wid(node_id, "filter"), tip)
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
fn checker_image() -> palantir::Image {
    const L: u8 = CHECKER_LIGHT_U8;
    const D: u8 = CHECKER_DARK_U8;
    let px = [
        [L, L, L, 255],
        [D, D, D, 255],
        [D, D, D, 255],
        [L, L, L, 255],
    ];
    palantir::Image::from_rgba8(2, 2, px.into_iter().flatten().collect())
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
        VIEWER_MIN_ZOOM,
        VIEWER_MAX_ZOOM,
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
    Text::new("1:1").style(&style).align(Align::CENTER).show(ui);
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
    // One arm per mode, so the match stays exhaustive over the enum itself
    // rather than over a flat-fill subset plus a wildcard that has to
    // re-reject the one mode it already handled.
    match mode {
        ViewerBackground::Theme => filled_rect(ui, rect, 2.0, theme.colors.canvas_bg),
        ViewerBackground::Black => filled_rect(ui, rect, 2.0, Color::BLACK),
        ViewerBackground::White => filled_rect(ui, rect, 2.0, Color::WHITE),
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
    }
    // Ring on top so the checker quads can't cover it.
    let (ring, width) = if selected {
        (theme.colors.selection_rect, 2.0)
    } else {
        (theme.colors.text_muted.with_alpha(0.4), 1.0)
    };
    stroked_rect(ui, rect, 2.0, ring, width);
}

/// A texture's logical footprint when each texel occupies one physical pixel.
fn logical_image_size(texels: UVec2, display_scale: f32) -> Vec2 {
    texels.as_vec2() / display_scale
}

/// The pane-local rect a viewport paints the texture into.
fn draw_rect(img: Vec2, v: Viewport) -> Rect {
    Rect {
        min: v.pan,
        size: Size::new(img.x * v.zoom, img.y * v.zoom),
    }
}

/// Aspect-preserving fit of `img` (its 1:1 logical footprint) in `pane`
/// (`ImageFit::Contain` semantics, upscaling small images too), as an
/// explicit viewport so the drawn fit and the gesture math can't drift.
fn fit_viewport(img: Vec2, pane: Vec2) -> Viewport {
    let zoom = (pane.x / img.x).min(pane.y / img.y);
    Viewport {
        pan: (pane - img * zoom) * 0.5,
        zoom,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use palantir::Image as AptImage;
    use scenarium::NodeId;

    fn viewer_node() -> NodeId {
        NodeId::from_u128(1)
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

        // The same 400x200 fit in an 800x800 logical pane on a 2x display
        // occupies 1600x800 physical px, so its magnification is 4x while
        // the logical draw rect remains exactly 800x400.
        let img = logical_image_size(UVec2::new(400, 200), 2.0);
        assert_eq!(img, Vec2::new(200.0, 100.0));
        let v = fit_viewport(img, Vec2::new(800.0, 800.0));
        assert_eq!(v.zoom, 4.0);
        assert_eq!(v.pan, Vec2::new(0.0, 200.0));
        assert_eq!(draw_rect(img, v).size, Size::new(800.0, 400.0));
    }

    #[test]
    fn sync_source_refits_only_for_size_changes_or_removal() {
        let mut viewer = ImageViewer::new(viewer_node());
        viewer.view = Some(Viewport {
            pan: Vec2::ZERO,
            zoom: 3.0,
        });
        viewer.sync_source(Some(UVec2::new(2, 2)));
        assert!(
            viewer.view.is_none(),
            "first image establishes fresh framing"
        );

        viewer.view = Some(Viewport {
            pan: Vec2::new(4.0, 5.0),
            zoom: 2.0,
        });
        viewer.sync_source(Some(UVec2::new(2, 2)));
        assert_eq!(
            viewer.view,
            Some(Viewport {
                pan: Vec2::new(4.0, 5.0),
                zoom: 2.0,
            }),
            "same-size revisions preserve inspection framing"
        );

        viewer.sync_source(Some(UVec2::new(3, 1)));
        assert!(viewer.view.is_none(), "dimension changes refit");
        viewer.view = Some(Viewport {
            pan: Vec2::ZERO,
            zoom: 4.0,
        });
        viewer.sync_source(None);
        assert!(viewer.view.is_none(), "removing the source clears framing");
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

        // At 2x display scale, 1:1 physical magnification draws each texel
        // into half a logical pixel, which composes back to one physical px.
        let img_2x = logical_image_size(UVec2::new(400, 200), 2.0);
        let fit_2x = fit_viewport(img_2x, Vec2::new(800.0, 800.0));
        let v = zoom_about_pane_center(fit_2x, 1.0, pane);
        assert_eq!(v.pan, Vec2::new(300.0, 350.0));
        assert_eq!(draw_rect(img_2x, v).size, Size::new(200.0, 100.0));
    }

    #[test]
    fn checker_image_is_one_2x2_period() {
        let img = checker_image();
        const L: u8 = CHECKER_LIGHT_U8;
        const D: u8 = CHECKER_DARK_U8;
        // Row-major light/dark, dark/light — one full checker period.
        #[rustfmt::skip]
        let expected = [
            L, L, L, 255,  D, D, D, 255,
            D, D, D, 255,  L, L, L, 255,
        ];
        assert_eq!(img, AptImage::from_rgba8(2, 2, expected.to_vec()));
    }
}
