//! What the driver reports of the editor as text: [`State`] and [`UiList`].

use glam::Vec2;
use palantir::InspectedWidget;
use palantir::{Rect, Sense, Ui};
use scenarium::{
    Binding, GhostSource, Graph, GraphId, InputPort, NodeDecl, NodeId, NodeTriggers, OutputPort,
    SavedPort, Scope, TriggerPort,
};
use serde::Serialize;

use crate::core::document;
use crate::core::document::Document;
use crate::gui::app::App;
use crate::gui::pane::graph_ui::wid;
use crate::gui::port_ref::{PortKind, PortRef};

/// The characters of a widget's text a [`UiList`] keeps: a scroll area's text is all its rows.
pub const TEXT_CUT: usize = 120;
/// The nodes a [`State`] lists of each graph: a detailed report of this many is about 10,000
/// tokens, well inside what an agent's client takes in one answer.
pub const NODE_LIMIT: usize = 100;

/// How much a [`State`] holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detail {
    /// Each graph's nodes by name and where they are, its selection, the focus, the status and
    /// the undo.
    Concise,
    /// Also each node's id, ports, bindings and run status, and each graph's wires and camera.
    Detailed,
}

/// The editor as an agent reads it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct State {
    /// The open document's file, `None` while it is unsaved.
    pub document: Option<String>,
    pub dirty: bool,
    /// Each graph of the document: the main graph first, then the compounds in id order.
    pub graphs: Vec<GraphSummary>,
    /// The text of the focused widget, or its id when it shows none.
    pub focus: Option<String>,
    /// The status bar's message.
    pub status: Option<String>,
    /// The warnings and errors logged since the last report.
    pub log: Vec<String>,
    pub can_undo: bool,
}

/// One graph of the document.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GraphSummary {
    /// `main`, or the compound's name.
    pub name: String,
    /// Whether the window's commands act on this graph.
    pub active: bool,
    /// The nodes whose name holds the report's filter, by name, at most [`NODE_LIMIT`] of them.
    pub nodes: Vec<NodeSummary>,
    /// Matching nodes past the limit.
    pub omitted: usize,
    /// The selected nodes, by name.
    pub selection: Vec<String>,
    /// The nodes the graph keeps and cannot hold, with what they were.
    pub ghost_nodes: Vec<GhostNodeSummary>,
    /// The wiring the graph keeps and cannot hold, each as
    /// `<node>.<output> -> <node>.<input>`, `const <value> -> <node>.<input>` or
    /// `<node>.<event> => <node>`, a port no declaration gives by its saved name or number.
    pub ghosts: Vec<String>,
    pub details: Option<Details>,
}

/// One node a graph keeps and cannot hold.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GhostNodeSummary {
    pub name: String,
    /// Why the load took it out.
    pub reason: String,
    /// What its func was called, where a save kept it.
    pub was: Option<String>,
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
    /// How its last run went, in the instance its graph's pane shows.
    pub run: Option<String>,
}

/// One input port.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PortSummary {
    pub name: String,
    /// `wire from <node>.<output>`, `const <value>`, or `None` when unbound.
    pub binding: Option<String>,
}

/// What a detailed report adds about a whole graph.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Details {
    /// Every wire with a listed node's name at either end, `<node>.<output> -> <node>.<input>`.
    pub wires: Vec<String>,
    pub pan: Vec2,
    pub zoom: f32,
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
    /// `app` as its last frame shows it in `ui`, with `log` as the lines since the last report,
    /// keeping the nodes whose name contains `nodes`, when given.
    pub(crate) fn of(
        app: &App,
        ui: &Ui,
        detail: Detail,
        nodes: Option<&str>,
        log: Vec<String>,
    ) -> Self {
        let open = app.open_document();
        let document = open.document();
        let mut widgets = Vec::new();
        ui.inspect(&mut widgets);
        let report = Report {
            app,
            ui,
            document,
            detailed: detail == Detail::Detailed,
            nodes,
        };
        Self {
            document: open.path().map(|path| path.display().to_string()),
            dirty: open.is_dirty(),
            graphs: document
                .graphs
                .graphs()
                .map(|(graph, _)| report.graph(graph))
                .collect(),
            focus: ui.focus().map(|id| {
                widgets
                    .iter()
                    .find(|widget| widget.id == id && !widget.text.is_empty())
                    .map_or_else(|| format!("{id:?}"), |widget| widget.text.clone())
            }),
            status: app.status().cur().map(|message| message.line.to_string()),
            log,
            can_undo: open.can_undo(),
        }
    }

    /// The graph the window's commands act on.
    pub fn active(&self) -> &GraphSummary {
        self.graphs
            .iter()
            .find(|graph| graph.active)
            .expect("one graph is active")
    }
}

/// What one [`State`] reads, shared by each graph it reports.
#[derive(Debug)]
struct Report<'a> {
    app: &'a App,
    ui: &'a Ui,
    document: &'a Document,
    detailed: bool,
    nodes: Option<&'a str>,
}

impl Report<'_> {
    fn graph(&self, id: GraphId) -> GraphSummary {
        let Self {
            app,
            ui,
            document,
            detailed,
            nodes,
        } = *self;
        let graph = document.graphs.graph(id);
        let scope = document.graphs.scope(app.library(), id);
        let view = document.view(id);
        let through = document.through(id);
        let listed = |name: &str| nodes.is_none_or(|part| name.contains(part));
        // By name, then id, as a reader looks a node up by its name.
        let mut matching: Vec<_> = graph
            .iter()
            .filter(|node| listed(document::title(node, scope)))
            .collect();
        matching.sort_unstable_by(|a, b| {
            (document::title(a, scope), a.node_id).cmp(&(document::title(b, scope), b.node_id))
        });
        GraphSummary {
            name: match id {
                GraphId::Main => "main".to_string(),
                GraphId::Compound(compound) => document
                    .graphs
                    .compound(compound)
                    .expect("the set holds each graph it lists")
                    .name
                    .clone(),
            },
            active: document.active == id,
            omitted: matching.len().saturating_sub(NODE_LIMIT),
            nodes: matching
                .into_iter()
                .take(NODE_LIMIT)
                .map(|node| {
                    let node_id = node.node_id;
                    let func = scope.resolve(&node);
                    let mut state = NodeSummary {
                        name: document::title(&node, scope).to_owned(),
                        func: func
                            .map_or_else(|why| format!("{why:?}"), |func| func.name().to_string()),
                        screen: ui.response_for(wid::body(id, node_id)).rect.map(rect),
                        id: None,
                        inputs: Vec::new(),
                        outputs: Vec::new(),
                        run: None,
                    };
                    if detailed {
                        state.id = Some(node_id.to_string());
                        if let Ok(func) = func {
                            state.inputs = func
                                .inputs()
                                .iter()
                                .map(|input| PortSummary {
                                    name: input.name.clone(),
                                    binding: binding(
                                        graph,
                                        scope,
                                        InputPort::new(node_id, input.id),
                                    ),
                                })
                                .collect();
                            state.outputs = func
                                .outputs()
                                .iter()
                                .map(|output| output.name.clone())
                                .collect();
                        }
                        state.run = through
                            .and_then(|path| app.run_state().node(path.node(node_id)))
                            .map(|node| format!("{:?}", node.status()));
                    }
                    state
                })
                .collect(),
            selection: view
                .selected
                .iter()
                .filter_map(|item| {
                    graph
                        .find(item.node()?)
                        .map(|node| document::title(node, scope).to_owned())
                })
                .collect(),
            ghost_nodes: graph
                .ghost_nodes()
                .filter(|(_, ghost)| listed(&ghost.node.name))
                .map(|(_, ghost)| GhostNodeSummary {
                    name: ghost.node.name.clone(),
                    reason: ghost.reason.to_string(),
                    was: ghost.saved.as_ref().map(|saved| saved.name.clone()),
                })
                .collect(),
            ghosts: ghosts(graph, scope),
            details: detailed.then(|| Details {
                wires: wires(graph, scope, listed),
                pan: view.viewport.pan,
                zoom: view.viewport.zoom,
            }),
        }
    }
}

impl UiList {
    /// The first `limit` of `matching`, the visible widgets a filter kept, in paint order.
    pub(crate) fn of(matching: &[InspectedWidget], limit: usize) -> Self {
        Self {
            omitted: matching.len().saturating_sub(limit),
            widgets: matching
                .iter()
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
fn output_name(graph: &Graph, scope: Scope<'_>, output: OutputPort) -> String {
    let Some(node) = graph.find(output.node_id) else {
        return format!("{}.{}", output.node_id, output.port);
    };
    let port = scope
        .resolve(node)
        .ok()
        .and_then(|func| func.ports().output(output.port))
        .map_or_else(|| output.port.to_string(), |port| port.name.clone());
    format!("{}.{port}", document::title(node, scope))
}

fn binding(graph: &Graph, scope: Scope<'_>, input: InputPort) -> Option<String> {
    graph
        .input_bindings(input.node_id)
        .find(|(port, _)| *port == input)
        .map(|(_, binding)| match binding {
            Binding::Wire(output) => format!("wire from {}", output_name(graph, scope, *output)),
            Binding::Const(value) => format!("const {value:?}"),
        })
}

/// A node of `graph` by its title, a ghost node's included.
fn node_title<'a>(graph: &'a Graph, scope: Scope<'a>, node_id: NodeId) -> &'a str {
    match graph.find(node_id) {
        Some(node) => document::title(node, scope),
        None => graph
            .ghost_node(node_id)
            .map_or("?", |ghost| ghost.node.name.as_str()),
    }
}

/// A port of `graph` by `<node>.<name>`, its name as [`Graph::shown_input`] finds it, else its
/// number.
fn ghost_end(graph: &Graph, scope: Scope<'_>, port: PortRef, saved: Option<&SavedPort>) -> String {
    let shown = match port.kind {
        PortKind::Input => graph.shown_input(scope, InputPort::new(port.node_id, port.port), saved),
        PortKind::Output => {
            graph.shown_output(scope, OutputPort::new(port.node_id, port.port), saved)
        }
    };
    let title = node_title(graph, scope, port.node_id);
    match shown {
        Some(shown) => format!("{title}.{}", shown.name),
        None => format!("{title}.#{}", port.port),
    }
}

/// Where a subscription lands, by name: a node's own trigger by the node's title, an `Outputs`
/// node's trigger or another id as `<node>.#<id>`.
fn trigger_name(graph: &Graph, scope: Scope<'_>, trigger: TriggerPort) -> String {
    let title = node_title(graph, scope, trigger.node_id);
    let rows = graph
        .find(trigger.node_id)
        .and_then(|node| scope.resolve(node).ok())
        .is_some_and(|decl| matches!(decl.triggers(), NodeTriggers::PerEvent(_)));
    if trigger.trigger == TriggerPort::NODE && !rows {
        title.to_owned()
    } else {
        format!("{title}.#{}", trigger.trigger)
    }
}

/// Every ghost of `graph`, sorted: the ghost wires and constants, then the ghost subscriptions.
fn ghosts(graph: &Graph, scope: Scope<'_>) -> Vec<String> {
    let wiring = graph.ghost_wiring();
    let mut ghosts: Vec<String> = wiring
        .bindings()
        .map(|(input, ghost)| {
            let to = ghost_end(graph, scope, PortRef::from(input), ghost.input.as_ref());
            match &ghost.source {
                GhostSource::Wire { output, saved } => format!(
                    "{} -> {to}",
                    ghost_end(graph, scope, PortRef::from(*output), saved.as_ref())
                ),
                GhostSource::Const(value) => format!("const {value:?} -> {to}"),
            }
        })
        .collect();
    ghosts.extend(wiring.subscriptions().map(|(subscription, ghost)| {
        let event = ghost.event.as_ref().map_or_else(
            || format!("#{}", subscription.event.event),
            |saved| saved.name.clone(),
        );
        format!(
            "{}.{event} => {}",
            node_title(graph, scope, subscription.event.node_id),
            trigger_name(graph, scope, subscription.trigger)
        )
    }));
    ghosts.sort_unstable();
    ghosts
}

/// The wires with a node whose name `listed` keeps at either end, sorted by their text, as a reader
/// reads them by name.
fn wires(graph: &Graph, scope: Scope<'_>, listed: impl Fn(&str) -> bool) -> Vec<String> {
    let mut wires = Vec::new();
    for node in graph.iter() {
        let ports = scope.resolve(&node).ok().map(NodeDecl::ports);
        for (port, binding) in graph.input_bindings(node.node_id) {
            if let Binding::Wire(output) = binding
                && (listed(document::title(&node, scope))
                    || graph
                        .find(output.node_id)
                        .is_some_and(|producer| listed(document::title(producer, scope))))
            {
                let input = ports
                    .and_then(|ports| ports.input(port.port))
                    .map_or_else(|| port.port.to_string(), |input| input.name.clone());
                wires.push(format!(
                    "{} -> {}.{input}",
                    output_name(graph, scope, *output),
                    document::title(&node, scope)
                ));
            }
        }
    }
    wires.sort_unstable();
    wires
}

#[cfg(test)]
mod tests {
    use scenarium::GraphId;

    use crate::core::document::internals::DocumentFixture;
    use crate::driver::state::{trigger_name, wires};

    /// The wires come sorted, whatever the graph's order; a filter keeps those with a listed
    /// node at either end.
    #[test]
    fn the_wires_are_sorted_and_filtered_by_either_end() {
        let fixture = DocumentFixture::sample();
        let graphs = &fixture.document.graphs;
        let (graph, scope) = (graphs.main(), graphs.scope(&fixture.library, GraphId::Main));
        // The sample wires get_a and get_b into sum, sum and get_b into mult, and mult into Print.
        assert_eq!(
            wires(graph, scope, |_| true),
            [
                "get_a.out0 -> sum.in0",
                "get_b.out0 -> mult.in1",
                "get_b.out0 -> sum.in1",
                "mult.out0 -> Print.in0",
                "sum.out0 -> mult.in0",
            ]
        );
        assert_eq!(
            wires(graph, scope, |name| name == "sum"),
            [
                "get_a.out0 -> sum.in0",
                "get_b.out0 -> sum.in1",
                "sum.out0 -> mult.in0",
            ]
        );
    }

    /// A subscription's end reads as its node's title on the node's own trigger, and with the id
    /// on an `Outputs` node's, whose id 1 is an interface event's, and on an id the node lacks.
    #[test]
    fn a_trigger_reads_as_its_node_or_with_its_id() {
        use scenarium::internals::stub_func;
        use scenarium::{
            Compound, CompoundId, EventDecl, EventPorts, FuncId, GraphSet, Interface, Library,
            Node, NodeId, Numbered, PortId, TriggerPort,
        };

        let probe = stub_func(FuncId::unique(), "probe").sink().build();
        let mut library = Library::default();
        library.add(probe.clone());
        let interface = Interface::default()
            .with_event_ports(EventPorts {
                events: vec![Numbered::new(PortId::new(1), EventDecl::new("done"))],
                trigger: None,
            })
            .unwrap();
        let mut compound = Compound::new("relay", interface);
        let outputs = compound.outputs_node();
        let sink = NodeId::unique();
        compound.graph.insert(sink, Node::from(&probe));
        let id = CompoundId::from_u128(1);
        let mut set = GraphSet::default();
        set.insert_compound(id, compound);
        let (graph, scope) = (
            set.graph(GraphId::Compound(id)),
            set.scope(&library, GraphId::Compound(id)),
        );
        let name = |trigger| trigger_name(graph, scope, trigger);
        assert_eq!(name(TriggerPort::from(sink)), "probe");
        assert_eq!(name(TriggerPort::new(sink, PortId::new(7))), "probe.#7");
        assert_eq!(
            name(TriggerPort::new(outputs, PortId::new(1))),
            "Outputs.#1"
        );
    }
}
