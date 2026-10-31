//! [`Shot`]: a picture of the window as a user's capture gives it, sized for an agent to read,
//! with a legend of where each node and port is in it.

use std::io::Cursor;
use std::path::PathBuf;

use glam::UVec2;
use image::ImageFormat;
use palantir::{DockView, Image, LocateError, Rect, Screenshot, Ui};
use serde::Serialize;

use crate::core::document::tab_ref::TabRef;
use crate::driver::error::DriverError;
use crate::driver::locator::{EditorTarget, Locator};
use crate::gui::app::App;
use crate::gui::app::view_capture;
use crate::gui::main_window::dock_panes;
use crate::gui::pane::graph_ui::wid;
use crate::gui::port_ref::PortRef;

/// The characters of a region's name a shot's file name keeps: with its number and kind, well
/// under the 255 bytes a file name may take.
const SLUG_NAME_CUT: usize = 64;

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
    /// Where the driver writes the PNG, in the session directory.
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
            Self::Node(node) => Ok(Some(
                Locator::App(EditorTarget::Node(node.clone()))
                    .find(app, ui)?
                    .rect
                    .inflated(NODE_MARGIN),
            )),
            Self::World(world) => {
                let point = |at| {
                    Locator::App(EditorTarget::World(at))
                        .find(app, ui)
                        .map(|found| found.point)
                };
                let min = point(world.min)?;
                let max = point(world.max())?;
                // Not `from_min_max`: a rect of negative size is a caller's empty region.
                let size = max - min;
                Ok(Some(Rect::new(min.x, min.y, size.x, size.y)))
            }
        }
    }

    /// The name a shot of this region is written under: lowercase letters, digits and dashes, a
    /// name cut at [`SLUG_NAME_CUT`] characters so the file name fits every file system.
    pub(crate) fn slug(&self) -> String {
        let named = |kind: &str, name: &str| {
            let mut slug = format!("{kind}-");
            slug.extend(name.chars().take(SLUG_NAME_CUT).map(|c| {
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
        .filter(|group| label(group.active_tab()).to_string() == name)
        .collect();
    let group = match groups.as_slice() {
        [one] => one,
        [] => {
            return Err(LocateError::NotFound {
                locator,
                nearest: layout
                    .active_tabs()
                    .map(|tab| label(tab).to_string())
                    .collect(),
            }
            .into());
        }
        _ => {
            return Err(LocateError::Ambiguous {
                locator,
                matches: groups
                    .iter()
                    .map(|group| format!("{:?}", group.id()))
                    .collect(),
            }
            .into());
        }
    };
    ui.response_for(DockView::pane_id(layout, group.id()))
        .rect
        .filter(|rect| !rect.is_paint_empty())
        .ok_or_else(|| LocateError::OffScreen { locator }.into())
}

/// A picture of a screenshot, fitted, and the legend of what it shows: what a backend hands the
/// driver, which encodes it.
#[derive(Debug)]
pub(crate) struct Capture {
    picture: Image,
    legend: Vec<LegendNode>,
}

impl Capture {
    /// `picture`, a fit of `screenshot`, with the legend of `app`'s nodes as `ui`'s frame lays
    /// them out, placed in it.
    pub(crate) fn of(screenshot: &Screenshot, picture: Image, app: &App, ui: &Ui) -> Self {
        let legend = legend(screenshot, picture.size(), app, ui);
        Self { picture, legend }
    }
}

impl Shot {
    /// `capture` as a PNG, which `path` names.
    pub(crate) fn new(capture: Capture, path: PathBuf) -> Self {
        let size = capture.picture.size();
        let mut png = Vec::new();
        view_capture::rgba_image(&capture.picture)
            .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
            .expect("a PNG encodes into memory");
        Self {
            png,
            path,
            size,
            legend: capture.legend,
        }
    }
}

/// The visible nodes of `app`, as `ui`'s frame lays them out, each placed in a picture of
/// `picture` texels that fits `screenshot`.
fn legend(screenshot: &Screenshot, picture: UVec2, app: &App, ui: &Ui) -> Vec<LegendNode> {
    let place = |rect: Option<Rect>| {
        let rect = rect.filter(|rect| !rect.is_paint_empty())?;
        let placed = screenshot.place(rect, picture)?;
        Some([placed.min.x, placed.min.y, placed.size.w, placed.size.h])
    };
    let library = app.library();
    let mut nodes = Vec::new();
    for node in app.open_document().document.graph.iter() {
        let Some(rect) = place(ui.response_for(wid::body(node.node_id)).rect) else {
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
                place(ui.response_for(wid::port_circle(port)).rect).map(|rect| LegendPort {
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
                .filter_map(|(idx, output)| port(PortRef::output(node.node_id, idx), &output.name))
                .collect();
        }
        nodes.push(legend);
    }
    nodes
}

#[cfg(test)]
mod tests {
    use crate::driver::shot::Region;

    /// A slug keeps a name's ASCII letters and digits in lower case, a dash for anything else,
    /// and the first 64 characters of a longer name; a region with no name is its kind.
    #[test]
    fn a_slug_names_the_region_in_a_file_name() {
        let long = "a".repeat(100);
        for (region, slug) in [
            (Region::Window, "window".to_string()),
            (
                Region::Node("Brightness / Contrast".to_string()),
                "node-brightness---contrast".to_string(),
            ),
            (
                Region::Pane("Graph é".to_string()),
                "pane-graph--".to_string(),
            ),
            (Region::Node(long), format!("node-{}", "a".repeat(64))),
        ] {
            assert_eq!(region.slug(), slug, "{region:?}");
        }
    }
}
