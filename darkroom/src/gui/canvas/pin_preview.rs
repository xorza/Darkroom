//! The pinned-output preview widget's own look: a small card (fixed
//! footprint, a header bar over a content area) plus its uploaded-texture
//! cache. Sibling of [`crate::gui::canvas::pin_ui`], which owns the wire,
//! the port-circle glyph, and the drag gesture — this file only knows how
//! to paint the card and keep its thumbnail texture current.
//!
//! The card borrows a node body's fill/corner-radius/shadow so it reads as
//! "a small floating surface" like the inspector panels, but its *border*
//! stays the neutral [`crate::gui::theme::Theme::colors`]`.node_border` —
//! not the port's data-type accent. The accent lives on the port-circle
//! glyph alone (see [`super::pin_ui`]); tinting the card's own outline too
//! doubled up the color right next to it and read as over-decorated.

use std::collections::HashMap;
use std::sync::Arc;

use aperture::{
    Background, Color, Configure, Corners, FontWeight, ImageFilter, ImageFit, ImageHandle, Panel,
    Sense, Shadow, Shape, Sizing, Spacing, Stroke, Text, TextStyle, TextWrap, Ui, WidgetId,
};
use glam::Vec2;
use scenarium::data::{CustomValue, DataType, DynamicValue};
use scenarium::graph::OutputPort;

use crate::gui::image_viewer::convert_image_value;
use crate::gui::theme::Theme;
use crate::gui::widgets::support::sized_text;

/// Fixed footprint of a pinned output's preview widget, canvas-world units —
/// a stable size regardless of content, so a drag never has to re-measure
/// and an image never grows the widget. An image letterboxes inside via
/// `Contain`; a non-image value's formatted text centers in the same frame.
pub(crate) const PREVIEW_WIDTH: f32 = 120.0;
pub(crate) const PREVIEW_HEIGHT: f32 = 90.0;

/// Longest side, in pixels, an image-typed pinned value is downscaled to
/// before upload — small enough to stay cheap with many simultaneous pins.
const PREVIEW_TEXTURE_DIM: u32 = 256;

/// Stroke width of the preview widget's own border — its rounded corners'
/// *inner* radius (the header bar's top corners, the image's bottom
/// corners) is `node_corner_radius` minus this, matching how a real node's
/// header follows its body stroke's inner edge.
const PREVIEW_BORDER_WIDTH: f32 = 1.0;

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

/// An uploaded preview texture, kept alive across frames (an `ImageHandle`
/// frees its GPU texture when its last clone drops) and reconverted only
/// when the pinned value it came from actually changed.
struct CachedPreview {
    /// Identity of the pinned value this texture was converted from — an
    /// `Arc::ptr_eq` hit against a fresh push skips a redundant
    /// reconvert+reupload.
    source: Arc<dyn CustomValue>,
    handle: ImageHandle,
}

impl std::fmt::Debug for CachedPreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedPreview")
            .field("source_type", &self.source.type_id())
            .field("handle", &self.handle)
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
    /// The current thumbnail texture for `port`'s pinned image value,
    /// converting + uploading fresh only when the value changed since last
    /// time (an `Arc` identity miss) or nothing's cached yet. `None` when
    /// `value` isn't a (decodable) image — the caller falls back to text.
    pub(crate) fn resolve(
        &mut self,
        ui: &Ui,
        port: OutputPort,
        value: &DynamicValue,
    ) -> Option<ImageHandle> {
        let DynamicValue::Custom(data) = value else {
            self.textures.remove(&port);
            return None;
        };
        if let Some(cached) = self.textures.get(&port)
            && Arc::ptr_eq(&cached.source, data)
        {
            return Some(cached.handle.clone());
        }
        let (image, _, _) = convert_image_value(value, PREVIEW_TEXTURE_DIM).ok()?;
        let handle = ui.register_image(image);
        self.textures.insert(
            port,
            CachedPreview {
                source: Arc::clone(data),
                handle: handle.clone(),
            },
        );
        Some(handle)
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
/// a content area — a letterboxed image if `texture` is `Some`, else
/// centered `text`. `border` is the card's own (neutral, or broken-red)
/// outline — never the port's data-type accent; that lives on the
/// port-circle glyph [`super::pin_ui`] paints separately. Senses `DRAG` so
/// it doubles as the reposition drag's grab target ([`pin_preview_wid`]).
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_widget(
    ui: &mut Ui,
    theme: &Theme,
    port: OutputPort,
    top_left: Vec2,
    title: &str,
    border: Color,
    texture: Option<&ImageHandle>,
    text: Option<&str>,
) {
    // Inner corners follow the border stroke's inner edge, like a real
    // node's header does relative to its own (wider) body stroke.
    let inner_r = (theme.node_corner_radius - PREVIEW_BORDER_WIDTH).max(0.0);
    Panel::vstack()
        .id(pin_preview_wid(port))
        .position(top_left)
        .size((Sizing::Fixed(PREVIEW_WIDTH), Sizing::Fixed(PREVIEW_HEIGHT)))
        .sense(Sense::DRAG)
        .background(
            Background::rounded(
                theme.colors.node_fill,
                Corners::all(theme.node_corner_radius),
            )
            .with_stroke(Stroke::solid(border, PREVIEW_BORDER_WIDTH))
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
                    if let Some(handle) = texture {
                        ui.add_shape(
                            Shape::image(handle.clone())
                                .fit(ImageFit::Contain)
                                .filter(ImageFilter::Linear),
                        );
                        // Rounds the image's square corners into the card's
                        // own fill on the two corners the header doesn't
                        // already round — a plain image fill ignores the
                        // panel's rounded background. Stroked in the
                        // card's own (neutral) border, matching its edge.
                        ui.add_shape(Shape::WindowedRect {
                            local_rect: None,
                            corners: Corners::new(0.0, 0.0, inner_r, inner_r),
                            fill: theme.colors.node_fill.into(),
                            stroke: Stroke::solid(border, PREVIEW_BORDER_WIDTH),
                        });
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
        });
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
}
