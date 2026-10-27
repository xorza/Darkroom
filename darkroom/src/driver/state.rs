//! What the driver reports of the editor as text: [`State`] and [`UiList`].

use palantir::InspectedWidget;
use palantir::{Rect, Sense, Ui};
use scenarium::{Binding, Func, Graph, Library, NodeId, OutputPort};
use serde::Serialize;

use crate::gui::app::App;
use crate::gui::pane::graph_ui::node_ui::wid;

/// The characters of a widget's text a [`UiList`] keeps: a scroll area's text is all its rows.
pub const TEXT_CUT: usize = 120;

/// How much a [`State`] holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detail {
    /// The nodes by name and where they are, the selection, the focus and the status.
    Concise,
    /// Also each node's id, ports, bindings and run status, the wires, the camera and the undo.
    Detailed,
}

/// The editor as an agent reads it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct State {
    /// The open document's file, `None` while it is unsaved.
    pub document: Option<String>,
    pub dirty: bool,
    pub nodes: Vec<NodeSummary>,
    /// The selected nodes, by name.
    pub selection: Vec<String>,
    /// The text of the focused widget, or its id when it shows none.
    pub focus: Option<String>,
    /// The status bar's message.
    pub status: Option<String>,
    /// The warnings and errors logged since the last report.
    pub log: Vec<String>,
    pub details: Option<Details>,
}

/// One node.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NodeSummary {
    pub name: String,
    /// The func it runs, or why none resolves.
    pub func: String,
    /// The body's visible rect in the window, `[x, y, w, h]` in logical pixels; `None` off screen.
    pub screen: Option<[f32; 4]>,
    pub id: Option<String>,
    pub inputs: Vec<PortSummary>,
    pub outputs: Vec<String>,
    /// How its last run went.
    pub run: Option<String>,
}

/// One input port.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PortSummary {
    pub name: String,
    /// `wire from <node>.<output>`, `const <value>`, or `None` when unbound.
    pub binding: Option<String>,
}

/// What a detailed report adds about the whole graph.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Details {
    /// Every wire, `<node>.<output> -> <node>.<input>`.
    pub wires: Vec<String>,
    pub pan: [f32; 2],
    pub zoom: f32,
    pub can_undo: bool,
}

/// The visible interactive widgets, as [`Ui::inspect`] lists them.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UiList {
    pub widgets: Vec<UiWidget>,
    /// Matching widgets past the limit.
    pub omitted: usize,
}

/// One interactive widget.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UiWidget {
    /// Its text, cut at [`TEXT_CUT`] characters.
    pub text: String,
    /// The length of the whole text, in characters.
    pub text_len: usize,
    /// `[x, y, w, h]` in logical pixels.
    pub rect: [f32; 4],
    pub clicks: bool,
    pub drags: bool,
    pub focusable: bool,
    pub disabled: bool,
}

impl State {
    /// `app` as its last frame shows it in `ui`, with `log` as the lines since the last report;
    /// a detailed report keeps the nodes whose name contains `nodes`, when given.
    pub(crate) fn of(
        app: &App,
        ui: &Ui,
        detail: Detail,
        nodes: Option<&str>,
        log: Vec<String>,
    ) -> Self {
        let open = app.open_document();
        let document = &open.document;
        let graph = &document.graph;
        let library = app.library();
        let detailed = detail == Detail::Detailed;
        let mut widgets = Vec::new();
        ui.inspect(&mut widgets);
        Self {
            document: open.path.as_ref().map(|path| path.display().to_string()),
            dirty: open.dirty,
            nodes: graph
                .iter()
                .filter(|node| !detailed || nodes.is_none_or(|part| node.name.contains(part)))
                .map(|node| {
                    let id = node.node_id;
                    let func = library.func_of(&node);
                    let mut state = NodeSummary {
                        name: node.name.clone(),
                        func: func
                            .map_or_else(|why| format!("{why:?}"), |func| func.name().to_string()),
                        screen: ui.response_for(wid::body(id)).rect.map(rect),
                        id: None,
                        inputs: Vec::new(),
                        outputs: Vec::new(),
                        run: None,
                    };
                    if detailed {
                        state.id = Some(id.to_string());
                        if let Ok(func) = func {
                            state.inputs = func
                                .inputs()
                                .iter()
                                .enumerate()
                                .map(|(idx, input)| PortSummary {
                                    name: input.name.clone(),
                                    binding: binding(graph, library, id, idx),
                                })
                                .collect();
                            state.outputs = func
                                .outputs()
                                .iter()
                                .map(|output| output.name.clone())
                                .collect();
                        }
                        state.run = app
                            .run_state()
                            .node(id)
                            .map(|node| format!("{:?}", node.status()));
                    }
                    state
                })
                .collect(),
            selection: document
                .main_view
                .selected
                .iter()
                .filter_map(|item| graph.find(item.node()?).map(|node| node.name.clone()))
                .collect(),
            focus: ui.focus().map(|id| {
                widgets
                    .iter()
                    .find(|widget| widget.id == id && !widget.text.is_empty())
                    .map_or_else(|| format!("{id:?}"), |widget| widget.text.clone())
            }),
            status: app.status().cur().map(|message| message.line.to_string()),
            log,
            details: detailed.then(|| Details {
                wires: wires(graph, library),
                pan: document.main_view.viewport.pan.to_array(),
                zoom: document.main_view.viewport.zoom,
                can_undo: open.can_undo(),
            }),
        }
    }
}

impl UiList {
    /// The visible interactive widgets of `ui`'s last frame whose text contains `filter`, ignoring
    /// case, at most `limit` of them in paint order.
    pub(crate) fn of(ui: &Ui, filter: Option<&str>, limit: usize) -> Self {
        let mut widgets = Vec::new();
        ui.inspect(&mut widgets);
        let filter = filter.map(str::to_lowercase);
        let matching: Vec<&InspectedWidget> = widgets
            .iter()
            .filter(|widget| !widget.clipped)
            .filter(|widget| {
                filter
                    .as_deref()
                    .is_none_or(|filter| widget.text.to_lowercase().contains(filter))
            })
            .collect();
        Self {
            omitted: matching.len().saturating_sub(limit),
            widgets: matching
                .into_iter()
                .take(limit)
                .map(|widget| UiWidget {
                    text: widget.text.chars().take(TEXT_CUT).collect(),
                    text_len: widget.text.chars().count(),
                    rect: rect(widget.rect),
                    clicks: widget.sense.contains(Sense::CLICK),
                    drags: widget.sense.contains(Sense::DRAG),
                    focusable: widget.focusable,
                    disabled: widget.disabled,
                })
                .collect(),
        }
    }
}

const fn rect(rect: Rect) -> [f32; 4] {
    [rect.min.x, rect.min.y, rect.size.w, rect.size.h]
}

/// `<node>.<output>`, by name.
fn output_name(graph: &Graph, library: &Library, output: OutputPort) -> String {
    let Some(node) = graph.find(output.node_id) else {
        return format!("{}.{}", output.node_id, output.port_idx);
    };
    let port = library
        .func_of(node)
        .ok()
        .and_then(|func| func.outputs().get(output.port_idx))
        .map_or_else(|| output.port_idx.to_string(), |port| port.name.clone());
    format!("{}.{port}", node.name)
}

fn binding(graph: &Graph, library: &Library, node_id: NodeId, idx: usize) -> Option<String> {
    graph
        .input_bindings(node_id)
        .find(|(port, _)| port.port_idx == idx)
        .map(|(_, binding)| match binding {
            Binding::Wire(output) => format!("wire from {}", output_name(graph, library, *output)),
            Binding::Const(value) => format!("const {value:?}"),
        })
}

fn wires(graph: &Graph, library: &Library) -> Vec<String> {
    let mut wires = Vec::new();
    for node in graph.iter() {
        let inputs = library.func_of(&node).ok().map(Func::inputs);
        for (port, binding) in graph.input_bindings(node.node_id) {
            if let Binding::Wire(output) = binding {
                let input = inputs
                    .and_then(|inputs| inputs.get(port.port_idx))
                    .map_or_else(|| port.port_idx.to_string(), |input| input.name.clone());
                wires.push(format!(
                    "{} -> {}.{input}",
                    output_name(graph, library, *output),
                    node.name
                ));
            }
        }
    }
    wires
}
