//! The pinned-output preview widget's own look: a small card (fixed
//! footprint, a header bar and — for an image — an info footer around the
//! content area) plus its uploaded-texture cache. Sibling of
//! [`crate::gui::canvas::pin_ui`], which owns the wire, the port-circle
//! glyph, and the drag gesture — this file only knows how to paint the
//! card and keep its thumbnail texture current.
//!
//! The card borrows a node body's fill/corner-radius/shadow so it reads as
//! "a small floating surface" like the inspector panels. Its *border* is
//! the neutral [`crate::gui::theme::Theme::colors`]`.node_border` at rest,
//! or the selection halo when selected — the same two-state border a node
//! body paints — but never the port's data-type accent. The accent lives on
//! the port-circle glyph alone (see [`super::pin_ui`]); tinting the card's
//! own outline too doubled up the color right next to it and read as
//! over-decorated.

use std::collections::HashMap;
use std::sync::Arc;

use aperture::{
    Align, Background, Color, Configure, Corners, FontWeight, ImageFilter, ImageFit, ImageHandle,
    Panel, Response, Sense, Shadow, Shape, Sizing, Spacing, Stroke, Text, TextStyle, TextWrap, Ui,
    VAlign, WidgetId,
};
use glam::{UVec2, Vec2};
use imaginarium::ColorFormat;
use scenarium::data::{CustomValue, DataType, DynamicValue, RamUsage};
use scenarium::graph::OutputPort;

use crate::gui::format::fmt_bytes;
use crate::gui::image_viewer::convert_image_value;
use crate::gui::theme::Theme;
use crate::gui::widgets::support::{footer_background, labeled_value, mono_text, sized_text};

/// Fixed footprint of a pinned output's preview widget, canvas-world units —
/// a stable size regardless of content, so a drag never has to re-measure
/// and an image never grows the widget. An image letterboxes inside via
/// `Contain`; a non-image value's formatted text centers in the same frame.
pub(crate) const PREVIEW_WIDTH: f32 = 240.0;
pub(crate) const PREVIEW_HEIGHT: f32 = 180.0;

/// Longest side, in pixels, an image-typed pinned value is downscaled to
/// before upload — small enough to stay cheap with many simultaneous pins.
const PREVIEW_TEXTURE_DIM: u32 = 256;

/// Whether `ty` is an image value — the pinned output's preview widget
/// shows a thumbnail for these, and its formatted text for everything else.
pub(crate) fn is_image_type(ty: &DataType) -> bool {
    matches!(ty, DataType::Custom(id) if *id == *lens::IMAGE_TYPE_ID)
}

/// A preview widget's title: the producing node's name, plus the output's
/// own name when it says something the node's doesn't (a node can have
/// several pinned outputs, so the port name disambiguates them).
pub(crate) fn preview_title(node_name: &str, output_name: &str) -> String {
    if output_name.is_empty() || output_name.eq_ignore_ascii_case(node_name) {
        node_name.to_owned()
    } else {
        format!("{node_name} \u{b7} {output_name}")
    }
}

/// A resolved image thumbnail: the uploaded texture plus the source facts
/// its info footer reports. Cheap to clone (an `ImageHandle` is an `Rc`
/// clone; the rest are small `Copy` values).
#[derive(Clone, Debug)]
pub(crate) struct ImagePreview {
    handle: ImageHandle,
    native_size: UVec2,
    native_format: ColorFormat,
    /// This value's resident memory, refreshed every [`PreviewCache::resolve`]
    /// call regardless of whether the texture itself was cached — cheap
    /// (`CustomValue::ram_bytes` just reads a stored size) and can't go stale
    /// the way a cached texture's *pixels* would if it weren't reconverted.
    ram: RamUsage,
}

/// An uploaded preview texture, kept alive across frames (an `ImageHandle`
/// frees its GPU texture when its last clone drops) and reconverted only
/// when the pinned value it came from actually changed.
struct CachedPreview {
    /// Identity of the pinned value this texture was converted from — an
    /// `Arc::ptr_eq` hit against a fresh push skips a redundant
    /// reconvert+reupload.
    source: Arc<dyn CustomValue>,
    preview: ImagePreview,
}

impl std::fmt::Debug for CachedPreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedPreview")
            .field("source_type", &self.source.type_id())
            .field("preview", &self.preview)
            .finish()
    }
}

/// Uploaded thumbnail textures for every pinned output currently showing
/// one, keyed by port.
#[derive(Default, Debug)]
pub(crate) struct PreviewCache {
    textures: HashMap<OutputPort, CachedPreview>,
}

impl PreviewCache {
    /// The current thumbnail for `port`'s pinned image value, converting +
    /// uploading fresh only when the value changed since last time (an
    /// `Arc` identity miss) or nothing's cached yet. `None` when `value`
    /// isn't a (decodable) image — the caller falls back to text.
    pub(crate) fn resolve(
        &mut self,
        ui: &Ui,
        port: OutputPort,
        value: &DynamicValue,
    ) -> Option<ImagePreview> {
        let DynamicValue::Custom(data) = value else {
            self.textures.remove(&port);
            return None;
        };
        let ram = data.ram_bytes();
        if let Some(cached) = self.textures.get(&port)
            && Arc::ptr_eq(&cached.source, data)
        {
            return Some(ImagePreview {
                ram,
                ..cached.preview.clone()
            });
        }
        let (image, native_size, native_format) =
            convert_image_value(value, PREVIEW_TEXTURE_DIM).ok()?;
        let handle = ui.register_image(image);
        let preview = ImagePreview {
            handle,
            native_size,
            native_format,
            ram,
        };
        self.textures.insert(
            port,
            CachedPreview {
                source: Arc::clone(data),
                preview: preview.clone(),
            },
        );
        Some(preview)
    }

    /// Drop cached textures for ports `keep` no longer includes (unpinned
    /// or removed) — otherwise a session that pins/unpins/deletes many
    /// nodes over time would leak textures indefinitely.
    pub(crate) fn prune(&mut self, keep: impl Fn(OutputPort) -> bool) {
        self.textures.retain(|&port, _| keep(port));
    }
}

/// Stable widget id for a pinned output's preview widget — the drag target
/// for repositioning it. Reconstructible from the port so
/// [`crate::gui::canvas::geometry::CanvasGeometry::rebuild`]-style polling
/// can read its response without a cache.
pub(crate) fn pin_preview_wid(port: OutputPort) -> WidgetId {
    WidgetId::from_hash(("graph.node.pin_preview", port.node_id, port.port_idx))
}

/// Paint one pinned output's preview widget: a header bar (the title) over
/// a content area, plus — for an image — an info footer below it reporting
/// resolution, format, and resident size. `border`/`border_width` are the
/// card's own outline (neutral, broken-red, or the selection halo) — never
/// the port's data-type accent; that lives on the port-circle glyph
/// [`super::pin_ui`] paints separately. Senses `CLICK | DRAG` so it doubles
/// as the reposition drag's grab target and the selection click target
/// ([`pin_preview_wid`]).
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_widget<'ui>(
    ui: &'ui mut Ui,
    theme: &Theme,
    port: OutputPort,
    top_left: Vec2,
    title: &str,
    border: Color,
    border_width: f32,
    image: Option<&ImagePreview>,
    text: Option<&str>,
) -> Response<'ui> {
    // Inner corners follow the border stroke's inner edge, like a real
    // node's header does relative to its own (wider) body stroke.
    let inner_r = (theme.node_corner_radius - theme.node_border_width).max(0.0);
    Panel::vstack()
        .id(pin_preview_wid(port))
        .position(top_left)
        .size((Sizing::Fixed(PREVIEW_WIDTH), Sizing::Fixed(PREVIEW_HEIGHT)))
        .sense(Sense::CLICK | Sense::DRAG)
        .background(
            Background::rounded(
                theme.colors.node_fill,
                Corners::all(theme.node_corner_radius),
            )
            .with_stroke(Stroke::solid(border, border_width))
            .with_shadow(Shadow::drop(
                theme.colors.node_ambient_shadow,
                Vec2::new(0.0, 3.0),
                8.0,
            )),
        )
        .show(ui, |ui| {
            Panel::hstack()
                .id_salt("header")
                .size((Sizing::FILL, Sizing::Hug))
                .padding(Spacing::xy(8.0, 5.0))
                .background(Background::rounded(
                    theme.colors.header_fill,
                    Corners::new(inner_r, inner_r, 0.0, 0.0),
                ))
                .show(ui, |ui| {
                    Text::new(title.to_owned())
                        .style(TextStyle {
                            weight: FontWeight::Bold,
                            ..sized_text(ui, 11.0)
                        })
                        .text_wrap(TextWrap::Wrap)
                        .show(ui);
                });
            Panel::vstack()
                .id_salt("content")
                .size((Sizing::FILL, Sizing::FILL))
                .show(ui, |ui| {
                    if let Some(image) = image {
                        // Content sits between the header and the info
                        // footer (both full-width strips), so its own
                        // corners are already interior — no rounding trick
                        // needed here, unlike a lone image with nothing
                        // below it.
                        ui.add_shape(
                            Shape::image(image.handle.clone())
                                .fit(ImageFit::Contain)
                                .filter(ImageFilter::Linear),
                        );
                    } else if let Some(text) = text {
                        Panel::vstack()
                            .id_salt("text")
                            .size((Sizing::FILL, Sizing::FILL))
                            .padding(Spacing::all(8.0))
                            .show(ui, |ui| {
                                Text::new(text.to_owned())
                                    .style(sized_text(ui, 11.0))
                                    .text_wrap(TextWrap::Wrap)
                                    .show(ui);
                            });
                    }
                });
            if let Some(image) = image {
                info_row(ui, theme, inner_r, image);
            }
        })
        .response
}

/// The pinned image's info footer: resolution, pixel format (channel
/// layout + bit depth), and resident size — styled like a node body's
/// memory footer ([`crate::gui::node::memory_row`]), so every read-only
/// fact strip in the app reads as the same kind of thing.
fn info_row(ui: &mut Ui, theme: &Theme, inner_r: f32, image: &ImagePreview) {
    Panel::hstack()
        .id_salt("info")
        .size((Sizing::FILL, Sizing::Hug))
        .padding(Spacing::xy(8.0, 5.0))
        .gap(10.0)
        .child_align(Align::v(VAlign::Center))
        // Round only the bottom corners so the strip seats into the card's
        // rounded bottom — the header rounds the top the same way.
        .background(footer_background(theme, inner_r))
        .show(ui, |ui| {
            bare_value(
                ui,
                format!("{}\u{d7}{}", image.native_size.x, image.native_size.y),
            );
            bare_value(ui, format_label(image.native_format));
            Panel::hstack()
                .id_salt("info_size")
                .size((Sizing::Hug, Sizing::Hug))
                .gap(4.0)
                .child_align(Align::v(VAlign::Center))
                .show(ui, |ui| {
                    labeled_value(ui, theme, "Size", fmt_bytes(image.ram.total()));
                });
        });
}

/// `"RGBA \u{b7} 8-bit"`-style shorthand for a pixel format: channel layout,
/// then per-channel bit depth.
fn format_label(format: ColorFormat) -> String {
    let bits = format.channel_size.byte_count() as u32 * 8;
    format!("{} \u{b7} {bits}-bit", format.channel_count)
}

/// A bare mono-styled value with no label — used where the value's own
/// shape (`1920×1080`, `RGBA · 8-bit`) already says what it is.
fn bare_value(ui: &mut Ui, text: String) {
    Text::new(text).style(mono_text(ui, 10.5)).show(ui);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_title_dedups_output_name_repeating_the_node_name() {
        assert_eq!(preview_title("Blend", "Blend"), "Blend");
        assert_eq!(preview_title("Blend", "Result"), "Blend \u{b7} Result");
        assert_eq!(preview_title("Blend", ""), "Blend");
    }

    #[test]
    fn format_label_reports_channel_layout_and_bit_depth() {
        assert_eq!(format_label(ColorFormat::RGBA_U8), "RGBA \u{b7} 8-bit");
        assert_eq!(format_label(ColorFormat::RGB_U16), "RGB \u{b7} 16-bit");
        assert_eq!(format_label(ColorFormat::L_F32), "L \u{b7} 32-bit");
    }
}
