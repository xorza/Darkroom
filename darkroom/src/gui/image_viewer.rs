//! Full-resolution viewer for a port's cached runtime image, shown in its
//! own singleton editor tab ([`TabRef::ImageViewer`]). Clicking an
//! inspector-panel thumbnail hands the port's held [`DynamicValue`] over
//! (the full value the worker already delivered for the open panel — an
//! `Arc` clone, so no recompute and no worker round-trip) and focuses the
//! tab; if the value is gone (superseded run), the pane says so instead.
//!
//! The heavy work — RGBA8 conversion of the full buffer and the texture
//! upload — happens lazily on the first draw after [`ImageViewer::present`],
//! since only the record pass holds the `Ui`. The buffer is CPU-resident by
//! construction: the worker's thumbnail generation already pulled it to the
//! CPU in place (`ImageBuffer` interior mutability), and a thumbnail is the
//! only way to get here.
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

/// The image-viewer tab's state: what it shows and how it's framed.
/// Owned by [`MainWindow`](crate::gui::main_window::MainWindow) like the
/// other top-level panes; content is runtime-only (never persisted).
#[derive(Default, Debug)]
pub(crate) struct ImageViewer {
    /// Source node's display name, for the header readout.
    title: String,
    /// Value handed over by [`Self::present`], converted + uploaded on the
    /// next draw (which has the `Ui` to register the texture).
    pending: Option<DynamicValue>,
    image: Option<ImageHandle>,
    /// Source dimensions before the texture-cap downscale.
    native_size: UVec2,
    /// Why the pane is empty, when it is.
    message: Option<String>,
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
    /// Swap the viewer's content: `value` is the port's held runtime value
    /// (`None` when it's no longer cached). Resets framing to fit; the
    /// conversion + upload run on the next draw.
    pub(crate) fn present(&mut self, title: String, value: Option<DynamicValue>) {
        self.title = title;
        self.image = None;
        self.native_size = UVec2::ZERO;
        self.view = None;
        self.pan_anchor = None;
        self.message = match value {
            Some(_) => None,
            None => Some(
                "value is no longer cached — run the graph and click the preview again".to_owned(),
            ),
        };
        self.pending = value;
    }

    /// Draw the viewer pane (the whole tab content). Converts + uploads a
    /// pending value first, then applies last frame's pan/zoom gestures,
    /// then paints image (or message) and the header readout.
    pub(crate) fn show(&mut self, ui: &mut Ui, theme: &Theme) {
        if let Some(value) = self.pending.take() {
            match render_full(&value) {
                Ok(rendered) => {
                    self.native_size = rendered.native_size;
                    self.image = Some(ui.register_image(rendered.image));
                }
                Err(message) => self.message = Some(message),
            }
        }
        self.apply_gestures(ui);

        let pane = pane_size(ui);
        Panel::zstack()
            .id(pane_wid())
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
                        let hint = self.message.as_deref().unwrap_or(
                            "click an image preview in a node's inspection panel to view it here",
                        );
                        Text::new(hint.to_owned())
                            .style(muted_style(theme, ui))
                            .align(Align::CENTER)
                            .show(ui);
                    }
                }
                self.header(ui, theme, pane);
            });
    }

    /// The top-left readout: source node, native dimensions, whether the
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
        let resp = ui.response_for(pane_wid());
        let Some(pane) = pane_size(ui) else {
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

/// Last frame's measured pane size, `None` before the first layout.
fn pane_size(ui: &Ui) -> Option<Vec2> {
    let size = ui.response_for(pane_wid()).layout_rect?.size;
    (size.w > 0.0 && size.h > 0.0).then(|| Vec2::new(size.w, size.h))
}

/// Stable id for the viewer pane (the gesture + layout anchor).
fn pane_wid() -> WidgetId {
    WidgetId::from_hash("image_viewer.pane")
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
}
