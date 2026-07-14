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

use aperture::{
    Align, Background, Color, Configure, Corners, CursorIcon, FontWeight, ImageFilter, ImageFit,
    ImageHandle, Justify, Panel, Response, Sense, Shape, Sizing, Spacing, Stroke, Text, TextStyle,
    TextWrap, Ui, VAlign, WidgetId,
};
use glam::{UVec2, Vec2};
use imaginarium::ColorFormat;
use scenarium::OutputPort;
use scenarium::{DataType, DynamicValue, RamUsage};

use crate::gui::format::fmt_bytes;
use crate::gui::node::header::Badge;
use crate::gui::run_state::PinnedOutputValue;
use crate::gui::theme::Theme;
use crate::gui::widgets::support::{
    CARD_FOOTER_PAD_X, CARD_FOOTER_PAD_Y, CARD_HEADER_PAD_X, CARD_HEADER_PAD_Y, footer_background,
    header_background, labeled_value, mono_text, sized_text,
};

/// Fixed footprint of a pinned output's preview widget, canvas-world units —
/// a stable size regardless of content, so a drag never has to re-measure
/// and an image never grows the widget. An image letterboxes inside via
/// `Contain`; a non-image value's formatted text centers in the same frame.
pub(crate) const PREVIEW_WIDTH: f32 = 280.0;
pub(crate) const PREVIEW_HEIGHT: f32 = 200.0;

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
    /// (`CustomValue::ram_bytes` just reads a stored size) and independent
    /// of the prepared raster's metadata.
    ram: RamUsage,
}

/// An uploaded preview texture, kept alive across frames (an `ImageHandle`
/// frees its GPU texture when its last clone drops) and re-uploaded only
/// when the pinned value it came from actually changed.
#[derive(Debug)]
struct CachedPreview {
    revision: u64,
    preview: ImagePreview,
}

/// Uploaded thumbnail textures for every pinned output currently showing
/// one, keyed by port.
#[derive(Default, Debug)]
pub(crate) struct PreviewCache {
    textures: HashMap<OutputPort, CachedPreview>,
}

impl PreviewCache {
    /// The current thumbnail for `port`'s pinned image value, uploading the
    /// already-prepared raster only when the centralized value's revision
    /// changed or nothing's cached yet. `None` when `value` isn't a decodable
    /// image — the caller falls back to text.
    pub(crate) fn resolve(
        &mut self,
        ui: &Ui,
        port: OutputPort,
        value: &PinnedOutputValue,
    ) -> Option<ImagePreview> {
        let DynamicValue::Custom(data) = &value.value else {
            self.textures.remove(&port);
            return None;
        };
        let ram = data.ram_bytes();
        if let Some(cached) = self.textures.get(&port)
            && cached.revision == value.revision
        {
            return Some(ImagePreview {
                ram,
                ..cached.preview.clone()
            });
        }
        let Some(rendered) = &value.preview else {
            self.textures.remove(&port);
            return None;
        };
        let image = aperture::Image::from_rgba8(
            rendered.image.width,
            rendered.image.height,
            rendered.image.pixels.clone(),
        );
        let handle = ui.register_image(image);
        let preview = ImagePreview {
            handle,
            native_size: rendered.native_size,
            native_format: rendered.native_format,
            ram,
        };
        self.textures.insert(
            port,
            CachedPreview {
                revision: value.revision,
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

/// Stable id for a pin preview's refresh chip. `pub(crate)` so the
/// canvas-level scan ([`crate::gui::canvas::pin_ui::emit_pin_refresh_clicks`])
/// can poll the click from last frame's response.
pub(crate) fn refresh_badge_wid(port: OutputPort) -> WidgetId {
    WidgetId::from_hash(("graph.node.pin_refresh_badge", port.node_id, port.port_idx))
}

/// Stable id for the card's image viewport. It senses hover without
/// capturing presses, leaving the parent card's click/drag gesture intact;
/// the navigation scan combines its hover with the parent's click.
pub(crate) fn preview_image_wid(port: OutputPort) -> WidgetId {
    WidgetId::from_hash(("graph.node.pin_preview_image", port.node_id, port.port_idx))
}

/// The header's refresh chip: re-run to the node this pin's output came
/// from and refresh the value it's showing — the pin's own version of a
/// node's play chip ([`crate::gui::node::header::play_chip`]), same
/// control framing and hover-to-"go" color swap. The click is read at
/// canvas level via [`refresh_badge_wid`] and translated into the run
/// command there (this file never names `AppCommand`).
fn refresh_chip(ui: &mut Ui, theme: &Theme, port: OutputPort) {
    let wid = refresh_badge_wid(port);
    let hovered = ui.response_for(wid).hovered;
    let color = if hovered {
        theme.colors.exec_executed_glow
    } else {
        theme.colors.text_muted
    };
    Badge::control(
        "\u{21bb}",
        color,
        false,
        wid,
        "Refresh — re-run to this node and update the preview",
    )
    .show(ui);
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
    runnable: bool,
) -> Response<'ui> {
    // Inner corners follow the border stroke's inner edge, like a real
    // node's header does relative to its own (wider) body stroke — see
    // `Theme::card_inner_radius`.
    let inner_r = theme.card_inner_radius();
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
            // Same ambient shadow a resting node body casts (see
            // `Theme::elevation_shadow`) — the card is meant to read as one
            // more instance of the same floating surface, not its own look.
            .with_shadow(theme.elevation_shadow(10.0)),
        )
        .show(ui, |ui| {
            Panel::hstack()
                .id_salt("header")
                .size((Sizing::FILL, Sizing::Hug))
                .padding(Spacing::xy(CARD_HEADER_PAD_X, CARD_HEADER_PAD_Y))
                .gap(4.0)
                .child_align(Align::v(VAlign::Center))
                .background(header_background(theme, inner_r))
                .show(ui, |ui| {
                    // Leads the band ahead of the title, like a node's own
                    // play chip — the one control here that *does*
                    // something rather than describing the card. Hidden
                    // when the owning node can't resolve as a run seed
                    // (disabled/instance/boundary/missing), matching the
                    // node header's own `runnable()` gate.
                    if runnable {
                        refresh_chip(ui, theme, port);
                    }
                    // Same size + weight as a node's title (`node::header::title`)
                    // — the theme's base text, bold — so the two header bars
                    // read as one style at two widths, not two independently
                    // tuned looks.
                    Text::new(title.to_owned())
                        .style(TextStyle {
                            weight: FontWeight::Bold,
                            ..ui.theme.text
                        })
                        .text_wrap(TextWrap::Wrap)
                        .show(ui);
                });
            let content = Panel::vstack()
                .id(preview_image_wid(port))
                .size((Sizing::FILL, Sizing::FILL))
                .sense(if image.is_some() {
                    Sense::HOVER
                } else {
                    Sense::NONE
                })
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
                            .child_align(Align::CENTER)
                            .justify(Justify::Center)
                            .show(ui, |ui| {
                                Text::new(text.to_owned())
                                    .style(sized_text(ui, 11.0))
                                    .text_wrap(TextWrap::Wrap)
                                    .show(ui);
                            });
                    }
                })
                .response
                .snapshot();
            if content.hovered {
                ui.set_cursor(CursorIcon::Pointer);
            }
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
        .padding(Spacing::xy(CARD_FOOTER_PAD_X, CARD_FOOTER_PAD_Y))
        .gap(10.0)
        .line_gap(4.0)
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
