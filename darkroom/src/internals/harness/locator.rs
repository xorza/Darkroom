//! [`Locator`]: where an action lands, named by what the editor shows rather than by pixels.

use glam::Vec2;
use palantir::internals::inspected_widget::InspectedWidget;
use palantir::{Rect, Ui};
use scenarium::NodeId;

use crate::gui::app::App;
use crate::gui::pane::graph_ui::canvas;
use crate::gui::pane::graph_ui::node_ui::{port_row, wid};
use crate::gui::port_ref::PortRef;
use crate::internals::harness::error::HarnessError;

/// How many candidates a `NotFound` lists.
const NEAREST: usize = 10;

/// A place in the window, by meaning first, then by text, then by position.
#[derive(Clone, Debug, PartialEq)]
pub enum Locator {
    /// A node's body, by its name or its id.
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
    /// The one visible interactive widget showing exactly this text.
    Text(String),
    /// A point of the window, in logical pixels.
    Screen(Vec2),
    /// Another locator's point, moved by this many logical pixels.
    Offset(Box<Self>, Vec2),
    /// The match at this index, from 0, of a `Node` or `Text` locator that matches several: nodes
    /// in the document's order, widgets in paint order, as an `Ambiguous` error lists them.
    Nth(Box<Self>, usize),
}

/// Which column of a node a port sits in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    In,
    Out,
}

impl Locator {
    /// The window point this locator names in `app`'s current frame: a widget's centre, or the
    /// point itself.
    pub(crate) fn point(&self, app: &App, ui: &Ui) -> Result<Vec2, HarnessError> {
        match self {
            Self::World(point) => {
                let canvas = ui.response_for(canvas::outer_wid()).rect.ok_or_else(|| {
                    HarnessError::OffScreen {
                        locator: format!("{self:?}"),
                    }
                })?;
                let viewport = app.open_document().document.main_view.viewport;
                Ok(canvas.min + viewport.pan + *point * viewport.zoom)
            }
            Self::Screen(point) => Ok(*point),
            Self::Offset(inner, by) => Ok(inner.point(app, ui)? + *by),
            Self::Node(_)
            | Self::NodeHeader(_)
            | Self::Port { .. }
            | Self::Text(_)
            | Self::Nth(..) => self.rect(app, ui).map(Rect::center),
        }
    }

    /// The visible rect, in the window's logical pixels, of the widget this locator names: a
    /// node's body, its title, a port circle, a text, or one of several of them. A point names
    /// no widget.
    pub(crate) fn rect(&self, app: &App, ui: &Ui) -> Result<Rect, HarnessError> {
        match self {
            Self::Node(node) => {
                self.visible(ui.response_for(wid::body(find_node(self, app, node)?)).rect)
            }
            Self::NodeHeader(node) => self.visible(
                ui.response_for(wid::rename(find_node(self, app, node)?))
                    .rect,
            ),
            Self::Port { node, side, port } => {
                let port = find_port(self, app, find_node(self, app, node)?, *side, port)?;
                self.visible(ui.response_for(port_row::port_circle_wid(port)).rect)
            }
            Self::Text(text) => find_text(self, ui, text),
            Self::Nth(inner, n) => {
                let not_found = |count: usize| HarnessError::NotFound {
                    locator: format!("{self:?}"),
                    nearest: vec![format!("{inner:?} has {count} matches")],
                };
                match inner.as_ref() {
                    Self::Node(node) => {
                        let ids = node_ids(app, node);
                        let id = ids.get(*n).ok_or_else(|| not_found(ids.len()))?;
                        self.visible(ui.response_for(wid::body(*id)).rect)
                    }
                    Self::Text(text) => {
                        let (_, matches) = text_matches(ui, text);
                        let widget = matches.get(*n).ok_or_else(|| not_found(matches.len()))?;
                        Ok(widget.rect)
                    }
                    _ => Err(HarnessError::NotFound {
                        locator: format!("{self:?}"),
                        nearest: vec!["Nth picks among Node or Text matches only".to_string()],
                    }),
                }
            }
            Self::World(_) | Self::Screen(_) | Self::Offset(..) => Err(HarnessError::NotFound {
                locator: format!("{self:?}"),
                nearest: vec!["a point names no widget".to_string()],
            }),
        }
    }

    /// `rect`, the visible rect of what this locator names; `None` is a widget that did not
    /// record or that its clip hides.
    fn visible(&self, rect: Option<Rect>) -> Result<Rect, HarnessError> {
        rect.filter(|rect| !rect.is_paint_empty())
            .ok_or_else(|| HarnessError::OffScreen {
                locator: format!("{self:?}"),
            })
    }
}

/// The one node named `node` by its name or its id.
fn find_node(locator: &Locator, app: &App, node: &str) -> Result<NodeId, HarnessError> {
    let graph = &app.open_document().document.graph;
    let matches = node_ids(app, node);
    match matches.as_slice() {
        [one] => Ok(*one),
        [] => Err(HarnessError::NotFound {
            locator: format!("{locator:?}"),
            nearest: nearest(graph.iter().map(|candidate| candidate.name.clone()), node),
        }),
        _ => Err(HarnessError::Ambiguous {
            locator: format!("{locator:?}"),
            matches: matches.iter().map(|id| format!("{node} ({id})")).collect(),
        }),
    }
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
    locator: &Locator,
    app: &App,
    node_id: NodeId,
    side: Side,
    port: &str,
) -> Result<PortRef, HarnessError> {
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
            return Err(HarnessError::NotFound {
                locator: format!("{locator:?}"),
                nearest: vec![format!("the node's func does not resolve: {why:?}")],
            });
        }
    };
    let idx = names
        .iter()
        .position(|name| name == port)
        .or_else(|| port.parse().ok().filter(|idx| *idx < names.len()))
        .ok_or_else(|| HarnessError::NotFound {
            locator: format!("{locator:?}"),
            nearest: names.clone(),
        })?;
    Ok(match side {
        Side::In => PortRef::input(node_id, idx),
        Side::Out => PortRef::output(node_id, idx),
    })
}

/// The rect of the one visible interactive widget showing exactly `text`.
fn find_text(locator: &Locator, ui: &Ui, text: &str) -> Result<Rect, HarnessError> {
    let (widgets, matches) = text_matches(ui, text);
    match matches.as_slice() {
        [one] => Ok(one.rect),
        [] => Err(HarnessError::NotFound {
            locator: format!("{locator:?}"),
            nearest: nearest(widgets.iter().map(|widget| widget.text.clone()), text),
        }),
        _ => Err(HarnessError::Ambiguous {
            locator: format!("{locator:?}"),
            matches: matches
                .iter()
                .enumerate()
                .map(|(n, widget)| format!("{n}: {:?} at {:?}", widget.text, widget.rect.min))
                .collect(),
        }),
    }
}

/// The visible interactive widgets, and those of them showing exactly `text`, in paint order.
fn text_matches(ui: &Ui, text: &str) -> (Vec<InspectedWidget>, Vec<InspectedWidget>) {
    let mut widgets = Vec::new();
    ui.inspect(&mut widgets);
    widgets.retain(|widget| !widget.clipped);
    let matches = widgets
        .iter()
        .filter(|widget| widget.text.trim() == text)
        .cloned()
        .collect();
    (widgets, matches)
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
