//! [`EditorTarget`]: the places only the editor can name, a node, its title, a port and a point
//! of the canvas, which palantir's [`Locator`] carries beside a widget's text and a window point.

use glam::Vec2;
use palantir::{Found, Locate, LocateError, Rect, Ui};
use scenarium::NodeId;

use crate::gui::app::App;
use crate::gui::pane::graph_ui::canvas;
use crate::gui::pane::graph_ui::wid;
use crate::gui::port_ref::PortRef;

/// How many candidates a `NotFound` lists.
const NEAREST: usize = 10;

/// A place in the window, by what the editor shows: palantir's locator with the editor's places.
pub type Locator = palantir::Locator<EditorTarget>;

/// A place the editor names, by meaning rather than by text or position.
#[derive(Clone, Debug, PartialEq)]
pub enum EditorTarget {
    /// A node's body, by its name or its id; a press lands on the middle of its footer, where no
    /// widget takes it.
    Node(String),
    /// A node's title, which drags the node and double-clicks to rename it.
    NodeHeader(String),
    /// A port circle: the node by name or id, the side, and the port by name or index.
    Port {
        node: String,
        side: Side,
        port: String,
    },
    /// A point of the graph canvas, in the canvas's own coordinates, where nodes are placed.
    World(Vec2),
}

/// Which column of a node a port sits in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    In,
    Out,
}

/// Every place a target names in the frame `ui` ran of `app`: each node of that name, in the
/// document's order, so `Nth` picks among them.
impl Locate<App> for EditorTarget {
    fn find_all(&self, app: &App, ui: &Ui, out: &mut Vec<Found>) -> Result<(), LocateError> {
        let locator = || format!("{self:?}");
        match self {
            Self::World(point) => {
                let canvas = ui
                    .response_for(canvas::outer_wid())
                    .rect
                    .ok_or_else(|| LocateError::OffScreen { locator: locator() })?;
                let viewport = app.open_document().document.main_view.viewport;
                let at = canvas.min + viewport.pan + *point * viewport.zoom;
                out.push(Found {
                    rect: Rect::new(at.x, at.y, 0.0, 0.0),
                    point: at,
                });
            }
            Self::Node(node) | Self::NodeHeader(node) | Self::Port { node, .. } => {
                let ids = node_ids(app, node);
                if ids.is_empty() {
                    let graph = &app.open_document().document.graph;
                    return Err(LocateError::NotFound {
                        locator: locator(),
                        nearest: nearest(
                            graph.iter().map(|candidate| candidate.name.clone()),
                            node,
                        ),
                    });
                }
                for id in ids {
                    let found = match self {
                        // The footer's middle, where no widget takes the press, when it shows;
                        // the body's center can land on a port row's field.
                        Self::Node(_) => visible(ui.response_for(wid::body(id)).rect).map(|body| {
                            let footer = visible(ui.response_for(wid::footer(id)).rect);
                            Found {
                                rect: body,
                                point: footer.map_or_else(|| body.center(), Rect::center),
                            }
                        }),
                        Self::NodeHeader(_) => {
                            visible(ui.response_for(wid::rename(id)).rect).map(Found::rect)
                        }
                        Self::Port { side, port, .. } => {
                            let port = find_port(self, app, id, *side, port)?;
                            visible(ui.response_for(wid::port_circle(port)).rect).map(Found::rect)
                        }
                        Self::World(_) => unreachable!("matched above"),
                    };
                    out.extend(found);
                }
                if out.is_empty() {
                    return Err(LocateError::OffScreen { locator: locator() });
                }
            }
        }
        Ok(())
    }
}

/// `rect` when it shows: `None` is a widget that did not record or that its clip hides.
fn visible(rect: Option<Rect>) -> Option<Rect> {
    rect.filter(|rect| !rect.is_paint_empty())
}

/// The nodes named `node` by their name or their id, in the document's order.
fn node_ids(app: &App, node: &str) -> Vec<NodeId> {
    app.open_document()
        .document
        .graph
        .iter()
        .filter(|candidate| candidate.name == node || candidate.node_id.to_string() == node)
        .map(|candidate| candidate.node_id)
        .collect()
}

/// The port of `node_id` on `side` named `port`, or at that index.
fn find_port(
    target: &EditorTarget,
    app: &App,
    node_id: NodeId,
    side: Side,
    port: &str,
) -> Result<PortRef, LocateError> {
    let node = app
        .open_document()
        .document
        .graph
        .find(node_id)
        .expect("a node the locator found is in the graph");
    let names: Vec<String> = match app.library().func_of(node) {
        Ok(func) => match side {
            Side::In => func
                .inputs()
                .iter()
                .map(|input| input.name.clone())
                .collect(),
            Side::Out => func
                .outputs()
                .iter()
                .map(|output| output.name.clone())
                .collect(),
        },
        Err(why) => {
            return Err(LocateError::NotFound {
                locator: format!("{target:?}"),
                nearest: vec![format!("the node's func does not resolve: {why:?}")],
            });
        }
    };
    let idx = names
        .iter()
        .position(|name| name == port)
        .or_else(|| port.parse().ok().filter(|idx| *idx < names.len()))
        .ok_or_else(|| LocateError::NotFound {
            locator: format!("{target:?}"),
            nearest: names.clone(),
        })?;
    Ok(match side {
        Side::In => PortRef::input(node_id, idx),
        Side::Out => PortRef::output(node_id, idx),
    })
}

/// The candidates that contain `wanted`, ignoring case, or the first few when none does.
fn nearest(candidates: impl Iterator<Item = String>, wanted: &str) -> Vec<String> {
    let candidates: Vec<String> = candidates
        .filter(|candidate| !candidate.is_empty())
        .collect();
    let wanted = wanted.to_lowercase();
    let close: Vec<String> = candidates
        .iter()
        .filter(|candidate| candidate.to_lowercase().contains(&wanted))
        .take(NEAREST)
        .cloned()
        .collect();
    if close.is_empty() {
        candidates.into_iter().take(NEAREST).collect()
    } else {
        close
    }
}
