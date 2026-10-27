//! [`Shot`]: a picture of the window as a user's capture gives it, sized for an agent to read,
//! with a legend of where each node and port is in it.

use std::io::Cursor;
use std::path::PathBuf;

use glam::{UVec2, Vec2};
use image::imageops::{self, FilterType};
use image::{ImageFormat, Rgba, Rgba32FImage, RgbaImage};
use palantir::{DockView, Rect, RgbaF32, Screenshot, SrgbaU8, Ui};
use serde::Serialize;

use crate::core::document::tab_ref::TabRef;
use crate::driver::error::DriverError;
use crate::driver::locator::Locator;
use crate::gui::app::App;
use crate::gui::app::view_capture;
use crate::gui::main_window::dock_panes;
use crate::gui::pane::graph_ui::node_ui::{port_row, wid};
use crate::gui::port_ref::PortRef;

/// The space a node's shot leaves around its body, in logical pixels, so its wires' ends and
/// its shadow show.
pub const NODE_MARGIN: f32 = 24.0;

/// What a shot covers.
#[derive(Clone, Debug, PartialEq)]
pub enum Region {
    Window,
    /// The pane whose visible tab has this name, with its tab strip.
    Pane(String),
    /// A node's body, by its name or its id, and [`NODE_MARGIN`] around it.
    Node(String),
    /// A rect of the graph canvas, in the canvas's own coordinates, where nodes are placed.
    World(Rect),
}

/// A picture of the window.
#[derive(Clone, Debug, PartialEq)]
pub struct Shot {
    /// The picture as a PNG file holds it.
    pub png: Vec<u8>,
    /// Where the PNG was written, in the session directory.
    pub path: PathBuf,
    /// Width and height in the picture's pixels.
    pub size: UVec2,
    /// The visible nodes, in the document's order.
    pub legend: Vec<LegendNode>,
}

/// Where one node shows in a [`Shot`].
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LegendNode {
    pub name: String,
    /// The visible part of its body, `[x, y, w, h]` in the picture's pixels.
    pub rect: [f32; 4],
    pub inputs: Vec<LegendPort>,
    pub outputs: Vec<LegendPort>,
}

/// Where one visible port circle shows in a [`Shot`].
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LegendPort {
    pub name: String,
    /// `[x, y, w, h]` in the picture's pixels.
    pub rect: [f32; 4],
}

impl Region {
    /// The logical rect to ask a screenshot of, `None` for the whole window.
    pub(crate) fn rect(&self, app: &App, ui: &Ui) -> Result<Option<Rect>, DriverError> {
        match self {
            Self::Window => Ok(None),
            Self::Pane(name) => pane(app, ui, name).map(Some),
            Self::Node(node) => Locator::Node(node.clone())
                .rect(app, ui)
                .map(|rect| Some(rect.inflated(NODE_MARGIN))),
            Self::World(world) => {
                let min = Locator::World(world.min).point(app, ui)?;
                let max = Locator::World(world.max()).point(app, ui)?;
                // Not `from_min_max`: a rect of negative size is a caller's empty region.
                let size = max - min;
                Ok(Some(Rect::new(min.x, min.y, size.x, size.y)))
            }
        }
    }

    /// The name a shot of this region is written under: lowercase letters, digits and dashes.
    pub(crate) fn slug(&self) -> String {
        let named = |kind: &str, name: &str| {
            let mut slug = format!("{kind}-");
            slug.extend(name.chars().map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            }));
            slug
        };
        match self {
            Self::Window => "window".to_string(),
            Self::Pane(name) => named("pane", name),
            Self::Node(name) => named("node", name),
            Self::World(_) => "world".to_string(),
        }
    }
}

/// The pane rect of the one group whose visible tab is named `name`.
fn pane(app: &App, ui: &Ui, name: &str) -> Result<Rect, DriverError> {
    let document = &app.open_document().document;
    let layout = &document.layout;
    let locator = format!("{:?}", Region::Pane(name.to_string()));
    let label = |tab: TabRef| dock_panes::tab_label(document, tab);
    let groups: Vec<_> = layout
        .groups()
        .filter(|group| label(group.active_tab()) == name)
        .collect();
    let group = match groups.as_slice() {
        [one] => one,
        [] => {
            return Err(DriverError::NotFound {
                locator,
                nearest: layout
                    .active_tabs()
                    .map(|tab| label(tab).to_string())
                    .collect(),
            });
        }
        _ => {
            return Err(DriverError::Ambiguous {
                locator,
                matches: groups
                    .iter()
                    .map(|group| format!("{:?}", group.id))
                    .collect(),
            });
        }
    };
    ui.response_for(DockView::pane_id(layout, group.id))
        .rect
        .filter(|rect| !rect.is_paint_empty())
        .ok_or(DriverError::OffScreen { locator })
}

impl Shot {
    /// `screenshot` scaled so that its longer side is at most `max_side`, with the legend of
    /// `app`'s nodes as `ui`'s frame lays them out.
    pub(crate) fn of(
        screenshot: &Screenshot,
        max_side: u32,
        app: &App,
        ui: &Ui,
        path: PathBuf,
    ) -> Result<Self, DriverError> {
        let region = screenshot.region();
        let image = screenshot.image().ok_or_else(|| DriverError::EmptyRegion {
            region: format!("{region:?}"),
        })?;
        let physical = image.size();
        let picture = fit(&view_capture::rgba_image(image), max_side);
        let size = UVec2::new(picture.width(), picture.height());
        let mut png = Vec::new();
        picture
            .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
            .expect("a PNG encodes into memory");
        Ok(Self {
            png,
            path,
            size,
            legend: Legend {
                origin: (region.min * screenshot.scale()).floor(),
                scale: screenshot.scale(),
                factor: size.as_vec2() / physical.as_vec2(),
                bounds: Rect::new(0.0, 0.0, size.x as f32, size.y as f32),
            }
            .nodes(app, ui),
        })
    }
}

/// How a logical rect of the window maps into the picture: to physical pixels at `scale`, from
/// the physical `origin` the screenshot starts at, then by `factor` per axis to the picture.
#[derive(Clone, Copy, Debug)]
struct Legend {
    origin: Vec2,
    scale: f32,
    factor: Vec2,
    bounds: Rect,
}

impl Legend {
    fn nodes(self, app: &App, ui: &Ui) -> Vec<LegendNode> {
        let library = app.library();
        let mut nodes = Vec::new();
        for node in app.open_document().document.graph.iter() {
            let Some(rect) = self.place(ui.response_for(wid::body(node.node_id)).rect) else {
                continue;
            };
            let mut legend = LegendNode {
                name: node.name.clone(),
                rect,
                inputs: Vec::new(),
                outputs: Vec::new(),
            };
            if let Ok(func) = library.func_of(&node) {
                let port = |port: PortRef, name: &str| {
                    self.place(ui.response_for(port_row::port_circle_wid(port)).rect)
                        .map(|rect| LegendPort {
                            name: name.to_string(),
                            rect,
                        })
                };
                legend.inputs = func
                    .inputs()
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, input)| port(PortRef::input(node.node_id, idx), &input.name))
                    .collect();
                legend.outputs = func
                    .outputs()
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, output)| {
                        port(PortRef::output(node.node_id, idx), &output.name)
                    })
                    .collect();
            }
            nodes.push(legend);
        }
        nodes
    }

    /// The part of the logical `rect` the picture shows, in its pixels; `None` when none does.
    fn place(self, rect: Option<Rect>) -> Option<[f32; 4]> {
        let rect = rect.filter(|rect| !rect.is_paint_empty())?;
        let min = (rect.min * self.scale - self.origin) * self.factor;
        let max = (rect.max() * self.scale - self.origin) * self.factor;
        let placed = Rect::from_min_max(min, max).intersect(self.bounds)?;
        Some([placed.min.x, placed.min.y, placed.size.w, placed.size.h])
    }
}

/// `image` scaled down so that its longer side is `max_side`, each side rounded to the nearest
/// pixel, or as it is when it already fits. The filter averages in linear light, as the frame
/// blends: averaging encoded bytes darkens every edge between a light and a dark pixel. A tent
/// filter widened by the scale weighs every source pixel and adds no ringing at the hard edges a
/// UI is made of.
fn fit(image: &RgbaImage, max_side: u32) -> RgbaImage {
    let longer = image.width().max(image.height());
    if longer <= max_side {
        return image.clone();
    }
    let side = |len: u32| {
        let scaled =
            (u64::from(len) * u64::from(max_side) + u64::from(longer) / 2) / u64::from(longer);
        u32::try_from(scaled.max(1)).expect("a scaled side is at most `max_side`")
    };
    let linear = Rgba32FImage::from_fn(image.width(), image.height(), |x, y| {
        let [r, g, b, a] = image.get_pixel(x, y).0;
        let color = RgbaF32::from_srgba(SrgbaU8 { r, g, b, a });
        Rgba([color.r, color.g, color.b, color.a])
    });
    let scaled = imageops::resize(
        &linear,
        side(image.width()),
        side(image.height()),
        FilterType::Triangle,
    );
    RgbaImage::from_fn(scaled.width(), scaled.height(), |x, y| {
        let [r, g, b, a] = scaled.get_pixel(x, y).0;
        let bytes = RgbaF32::new(r, g, b, a).to_srgba();
        Rgba([bytes.r, bytes.g, bytes.b, bytes.a])
    })
}

#[cfg(test)]
mod tests {
    use image::{Rgba, RgbaImage};

    use crate::driver::shot::fit;

    /// A 4 × 2 image of black and white columns, black first, fits a `max_side` of 2 as 2 × 1.
    /// The tent, widened by 2, weighs the columns at its centre 3/4 each and the next ones out
    /// 1/4, over the columns the image has: the left pixel holds 3/4 white of 7/4, the right one
    /// 1/4 + 3/4 of 7/4. In linear light that is 3/7 and 4/7, which sRGB encodes as 175 and 199;
    /// a mean of the encoded bytes would give 109 and 146.
    #[test]
    fn a_scaled_shot_averages_in_linear_light() {
        let image = RgbaImage::from_fn(4, 2, |x, _| {
            let v = if x % 2 == 0 { 0 } else { 255 };
            Rgba([v, v, v, 255])
        });
        let scaled = fit(&image, 2);
        assert_eq!((scaled.width(), scaled.height()), (2, 1));
        assert_eq!(scaled.get_pixel(0, 0).0, [175, 175, 175, 255]);
        assert_eq!(scaled.get_pixel(1, 0).0, [199, 199, 199, 255]);
        assert_eq!(fit(&image, 4), image);
    }
}
