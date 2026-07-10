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
    Align, Background, Color, Configure, ImageFit, ImageHandle, Panel, PointerButton, Rect, Sense,
    Shape, Size, Sizing, Spacing, Text, TextStyle, Ui, WidgetId,
};
use glam::{UVec2, Vec2};
use imaginarium::{Preview, ProcessingContext};
use lens::Image as LensImage;
use scenarium::data::DynamicValue;
use scenarium::graph::NodeSearch;

use crate::core::document::{Document, PortKind, PortRef};
use crate::core::worker::RunId;
use crate::gui::canvas::pan_zoom::{scroll_to_zoom_factor, zoom_about};
use crate::gui::theme::Theme;

/// Longest texture side we upload. Conservative device
/// `max_texture_dimension_2d` (aperture doesn't expose the real limit);
/// larger images are area-downscaled to fit and the header says so.
const MAX_TEXTURE_DIM: usize = 8192;

/// Viewer zoom bounds — far wider than the canvas's: out to overview a
/// texture-capped 8k frame in a small pane, in for pixel peeping.
const MIN_ZOOM: f32 = 0.02;
const MAX_ZOOM: f32 = 32.0;

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
/// source dimensions it was derived from.
#[derive(Debug)]
struct RenderedImage {
    image: aperture::Image,
    native_size: UVec2,
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
            message: None,
            content_epoch: None,
            view: None,
            pan_anchor: None,
        }
    }

    /// Show `value` as this viewer's content, resetting the framing to
    /// fit — the preview-click path. `value` is the port's held runtime
    /// value from `epoch` (`None` when it's no longer cached, which shows
    /// a message and leaves the epoch unset so the next run's value still
    /// fills the pane).
    pub(crate) fn present(&mut self, title: String, value: Option<DynamicValue>, epoch: RunId) {
        self.view = None;
        self.pan_anchor = None;
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
                        self.view = None;
                        self.pan_anchor = None;
                    }
                    self.native_size = rendered.native_size;
                    self.image = Some(ui.register_image(rendered.image));
                }
                Err(message) => {
                    self.image = None;
                    self.native_size = UVec2::ZERO;
                    self.message = Some(message);
                }
            }
        }
        self.apply_gestures(ui);

        let pane = pane_size(ui, self.port);
        Panel::zstack()
            .id(pane_wid(self.port))
            .size((Sizing::FILL, Sizing::FILL))
            .sense(Sense::CLICK | Sense::DRAG | Sense::SCROLL | Sense::PINCH)
            .clip_rect()
            .background(Background {
                fill: theme.colors.canvas_bg.into(),
                ..Default::default()
            })
            .show(ui, |ui| {
                match (&self.image, pane) {
                    (Some(handle), Some(pane)) => {
                        let v = self
                            .view
                            .unwrap_or_else(|| fit_viewport(handle.size().as_vec2(), pane));
                        ui.add_shape(Shape::Image {
                            handle: handle.clone(),
                            local_rect: Some(draw_rect(handle.size().as_vec2(), v)),
                            fit: ImageFit::Fill,
                            tint: Color::WHITE,
                        });
                    }
                    // Pane not measured yet (first frame): let aperture fit it.
                    (Some(handle), None) => {
                        ui.add_shape(Shape::Image {
                            handle: handle.clone(),
                            local_rect: None,
                            fit: ImageFit::Contain,
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
        // A hugging chip on the canvas fill so the readout stays legible
        // over bright image regions.
        Panel::hstack()
            .id_salt("viewer_header")
            .size((Sizing::Hug, Sizing::Hug))
            .padding(Spacing::new(8.0, 4.0, 8.0, 4.0))
            .background(Background {
                fill: theme.colors.canvas_bg.with_alpha(0.8).into(),
                ..Default::default()
            })
            .show(ui, |ui| {
                Text::new(text).style(muted_style(theme, ui)).show(ui);
            });
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
            self.view = None;
            self.pan_anchor = None;
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
        assert_eq!(rendered.image.width, 2);
        assert_eq!(rendered.image.height, 1);
        assert_eq!(rendered.image.pixels, bytes);

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
    fn same_custom_value_is_arc_identity() {
        let a = image_value(1, 1);
        let clone = a.clone();
        let b = image_value(1, 1);
        assert!(same_custom_value(&a, &clone), "clone shares the Arc");
        assert!(!same_custom_value(&a, &b), "equal content, different Arc");
        assert!(!same_custom_value(&a, &DynamicValue::from(1i64)));
    }
}
