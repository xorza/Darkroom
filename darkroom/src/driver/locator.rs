//! [`EditorTarget`]: the places only the editor can name, a node, its title, a port and a point
//! of the canvas, which palantir's [`Locator`] carries beside a widget's text and a window point.

use glam::Vec2;
use palantir::{Found, Locate, LocateError, Rect, Ui};
use scenarium::{GraphId, NodeId, PortId};

use crate::core::document;
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

/// Every place a target names in the frame `ui` ran of `app`, in the active graph's pane: each
/// node of that name, in the document's order, so `Locator::nth` picks among them.
impl Locate<App> for EditorTarget {
    fn find_all(&self, app: &App, ui: &Ui, out: &mut Vec<Found>) -> Result<(), LocateError> {
        let locator = || format!("{self:?}");
        let document = app.open_document().document();
        let graph = document.active;
        match self {
            Self::World(point) => {
                let canvas = ui
                    .response_for(canvas::outer_wid(graph))
                    .rect
                    .ok_or_else(|| LocateError::OffScreen { locator: locator() })?;
                // The camera the canvas draws through, so a world point lands where it paints.
                let camera = document.view(graph).viewport.landed(ui.display());
                let at = canvas.min + camera.pan + *point * camera.zoom;
                out.push(Found {
                    rect: Rect::new(at.x, at.y, 0.0, 0.0),
                    point: at,
                });
            }
            Self::Node(node) | Self::NodeHeader(node) | Self::Port { node, .. } => {
                let ids = node_ids(app, graph, node);
                if ids.is_empty() {
                    return Err(LocateError::NotFound {
                        locator: locator(),
                        nearest: nearest(
                            document.graphs.graph(graph).iter().map(|candidate| {
                                document::title(
                                    &candidate,
                                    document.graphs.scope(app.library(), graph),
                                )
                                .to_owned()
                            }),
                            node,
                        ),
                    });
                }
                for id in ids {
                    let found = match self {
                        // The footer's middle, where no widget takes the press, when it shows;
                        // the body's center can land on a port row's field.
                        Self::Node(_) => {
                            visible(ui.response_for(wid::body(graph, id)).rect).map(|body| {
                                let footer = visible(ui.response_for(wid::footer(graph, id)).rect);
                                Found {
                                    rect: body,
                                    point: footer.map_or_else(|| body.center(), Rect::center),
                                }
                            })
                        }
                        Self::NodeHeader(_) => {
                            visible(ui.response_for(wid::rename(graph, id)).rect)
                                .map(Found::center_of)
                        }
                        Self::Port { side, port, .. } => {
                            let port = find_port(self, app, graph, id, *side, port)?;
                            visible(ui.response_for(wid::port_circle(graph, port)).rect)
                                .map(Found::center_of)
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

/// The nodes of `graph` named `node` by their title or their id, in the document's order.
fn node_ids(app: &App, graph: GraphId, node: &str) -> Vec<NodeId> {
    let graphs = &app.open_document().document().graphs;
    let scope = graphs.scope(app.library(), graph);
    graphs
        .graph(graph)
        .iter()
        .filter(|candidate| {
            document::title(candidate, scope) == node || candidate.node_id.to_string() == node
        })
        .map(|candidate| candidate.node_id)
        .collect()
}

/// The port of `node_id` of `graph` on `side` named `port`, or at that index.
fn find_port(
    target: &EditorTarget,
    app: &App,
    graph: GraphId,
    node_id: NodeId,
    side: Side,
    port: &str,
) -> Result<PortRef, LocateError> {
    let graphs = &app.open_document().document().graphs;
    let node = graphs
        .graph(graph)
        .find(node_id)
        .expect("a node the locator found is in the graph");
    let ports: Vec<(PortId, String)> = match graphs.scope(app.library(), graph).resolve(node) {
        Ok(func) => match side {
            Side::In => func
                .inputs()
                .iter()
                .map(|input| (input.id, input.name.clone()))
                .collect(),
            Side::Out => func
                .outputs()
                .iter()
                .map(|output| (output.id, output.name.clone()))
                .collect(),
        },
        Err(why) => {
            return Err(LocateError::NotFound {
                locator: format!("{target:?}"),
                nearest: vec![format!("the node's func does not resolve: {why:?}")],
            });
        }
    };
    let idx = ports
        .iter()
        .position(|(_, name)| name == port)
        .or_else(|| port.parse().ok().filter(|idx| *idx < ports.len()))
        .ok_or_else(|| LocateError::NotFound {
            locator: format!("{target:?}"),
            nearest: ports.iter().map(|(_, name)| name.clone()).collect(),
        })?;
    let (id, _) = ports[idx];
    Ok(match side {
        Side::In => PortRef::input(node_id, id),
        Side::Out => PortRef::output(node_id, id),
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
