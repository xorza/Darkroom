//! Resolved output types for authoring graphs, filled in whole rather than
//! asked one port at a time.

use hashbrown::HashMap;

use crate::graph::Graph;
use crate::graph::identity::OutputPort;
use crate::library::Library;
use crate::{DataType, StaticValue};

/// What decides one output port's type, as the graph reports it. The single hop
/// the walk below takes, named so the reading of a declaration stays a `Graph`
/// query ([`Graph::output_source`]) and the walking of it stays here.
#[derive(Debug)]
pub(super) enum OutputTypeSource {
    /// Settled by the declaration itself.
    Fixed(DataType),
    /// A wildcard mirroring an input bound to this producer port.
    Bind(OutputPort),
    /// A wildcard mirroring an input bound to a constant, with the type declared
    /// for that input beside it.
    Const {
        declared: DataType,
        value: StaticValue,
    },
    /// Nothing to go on — an unbound mirror, or a missing func.
    Unresolved,
}

/// The effective type of every output port of one authoring graph, filled by
/// [`update`](Self::update) and read back by [`get`](Self::get).
///
/// A *wildcard* output — a reroute or passthrough — reports the type of whatever
/// feeds the input it mirrors, so answering for one means walking the bindings
/// above it. A fixed output answers from its own declaration. Both go in here, so
/// a caller asking "what type comes out of this port" needs one lookup and no
/// case analysis.
///
/// Filling it in bulk is what lets [`get`](Self::get) take `&self` — a caller
/// mid-way through building something can read a port's type without threading a
/// `&mut` through — and it means a reroute chain several ports read is walked
/// once, since the walk's memo *is* the table this keeps.
///
/// The invariant is the caller's: the graph must have been
/// [`update`](Self::update)d since its last edit. A host refreshing per frame and
/// a compile filling one before it walks both satisfy it by construction.
#[derive(Debug, Default)]
pub struct OutputTypes {
    /// The walk's memo, which *is* the answer table: resolving one port records
    /// every port on its chain.
    ///
    /// `None` marks a port the current walk is still inside — the mark that turns
    /// a binding cycle into `Any` instead of a hang. Nothing outside a walk can
    /// observe one, so [`get`](Self::get) reads it as "no answer".
    resolved: HashMap<OutputPort, Option<DataType>>,
    /// The chain the current walk has taken, so its whole length is memoized
    /// with the answer it arrives at rather than just the port asked for.
    path: Vec<OutputPort>,
}

impl OutputTypes {
    /// Resolve every output port of `graph` into the table, discarding whatever
    /// the last call put there.
    ///
    /// Replacing rather than accumulating is what makes the table exactly one
    /// graph's: a port absent from `graph` is absent here afterwards, so a stale
    /// answer can't outlive the edit that invalidated it. The capacity survives,
    /// so a host refreshing per frame allocates once.
    pub fn update(&mut self, graph: &Graph, library: &Library) {
        self.resolved.clear();
        self.path.clear();
        for node in graph.iter() {
            let Some(ports) = graph.node_func(&node, library) else {
                continue;
            };
            for port_idx in 0..ports.outputs.len() {
                self.resolve(graph, library, OutputPort::new(node.id, port_idx));
            }
        }
    }

    /// The effective type at `port`, or `None` for a port the last
    /// [`update`](Self::update) did not cover.
    ///
    /// A port whose chain dead-ends or closes a cycle is present and
    /// [`Any`](DataType::Any) — absence never means "polymorphic", it means the
    /// port was not among the graph's declared outputs. The two callers read that
    /// differently on purpose: a host asking about a port it just read off an
    /// interface has a bug, while a compile asking about a port a stale document
    /// still names has *drift*, and `Any` is the right answer there.
    pub fn get(&self, port: OutputPort) -> Option<&DataType> {
        self.resolved.get(&port)?.as_ref()
    }

    /// Follow `port` up its wildcard chain until something settles it, recording
    /// the answer for every port on the way.
    ///
    /// Iterative, not recursive: the chain length is a user-authored reroute
    /// count, which must not decide the stack depth. A port already in the table
    /// ends the walk immediately, which is what makes covering a whole graph cost
    /// one walk per chain rather than one per port.
    fn resolve(&mut self, graph: &Graph, library: &Library, port: OutputPort) -> DataType {
        self.path.clear();
        let mut current = port;
        let data_type = loop {
            match self.resolved.get(&current) {
                // Already inside this walk: the chain closed on itself.
                Some(None) => break DataType::Any,
                Some(Some(data_type)) => break data_type.clone(),
                None => {}
            }
            self.resolved.insert(current, None);
            self.path.push(current);
            match graph.output_source(library, current) {
                OutputTypeSource::Fixed(data_type) => break data_type,
                OutputTypeSource::Bind(producer) => current = producer,
                OutputTypeSource::Const { declared, value } => {
                    break declared.or_const_type(&value);
                }
                OutputTypeSource::Unresolved => break DataType::Any,
            }
        };
        for resolved in self.path.drain(..) {
            self.resolved.insert(resolved, Some(data_type.clone()));
        }
        data_type
    }
}

#[cfg(test)]
mod tests {
    use crate::graph::func::{FuncInput, FuncOutput};
    use crate::graph::identity::{FuncId, InputPort, NodeId, OutputPort};
    use crate::graph::output_types::OutputTypes;
    use crate::graph::{Binding, Graph};
    use crate::library::Library;
    use crate::{DataType, Func, testing};

    /// `src` (declared `Int`) → `p1` → `p2`, both passthroughs declaring a
    /// wildcard output over an `Any` input.
    #[derive(Debug)]
    struct Chain {
        library: Library,
        graph: Graph,
        src: NodeId,
        p1: NodeId,
        p2: NodeId,
        /// A second producer, wired to nothing — its fixed output lies on no
        /// wildcard chain.
        isolated: NodeId,
    }

    fn chain() -> Chain {
        let producer = testing::with_stub_lambda(
            Func::new(FuncId::unique(), "src").output(FuncOutput::new("out", DataType::Int)),
        );
        let pass = testing::with_stub_lambda(
            Func::new(FuncId::unique(), "pass")
                .input(FuncInput::required("i", DataType::Any))
                .wildcard_output("o", 0),
        );
        let mut library = Library::default();
        library.add(producer.clone());
        library.add(pass.clone());

        let mut graph = Graph::default();
        let src = graph.add_func_node(&producer);
        let p1 = graph.add_func_node(&pass);
        let p2 = graph.add_func_node(&pass);
        let isolated = graph.add_func_node(&producer);
        graph.set_input_binding(InputPort::new(p1, 0), Binding::bind(src, 0));
        graph.set_input_binding(InputPort::new(p2, 0), Binding::bind(p1, 0));

        Chain {
            library,
            graph,
            src,
            p1,
            p2,
            isolated,
        }
    }

    /// Every declared output port is in the table, whichever kind it is: a
    /// wildcard through the chain above it, a fixed output from its own
    /// declaration. One lookup answers for both, which is what lets a caller
    /// skip the case analysis. A port that does not exist stays out.
    #[test]
    fn covers_every_declared_output_port() {
        let c = chain();
        let mut types = OutputTypes::default();

        types.update(&c.graph, &c.library);

        assert_eq!(
            types.get(OutputPort::new(c.p1, 0)),
            Some(&DataType::Int),
            "the passthrough mirrors the producer it reads"
        );
        assert_eq!(
            types.get(OutputPort::new(c.p2, 0)),
            Some(&DataType::Int),
            "transitively, through the chain"
        );
        assert_eq!(
            types.get(OutputPort::new(c.src, 0)),
            Some(&DataType::Int),
            "the fixed producer answers from its own declaration"
        );
        assert_eq!(
            types.get(OutputPort::new(c.isolated, 0)),
            Some(&DataType::Int),
            "including one no wildcard chain reaches"
        );
        assert_eq!(
            types.get(OutputPort::new(c.p1, 7)),
            None,
            "a port that does not exist is not declared, so not in the table"
        );
    }

    /// `update` is a replacement, not an accumulation: the previous graph's
    /// answers must not survive it. This is the invariant a per-frame refresh
    /// relies on, and the one a stale table would break silently.
    #[test]
    fn update_discards_what_the_previous_graph_resolved() {
        let mut c = chain();
        let mut types = OutputTypes::default();
        types.update(&c.graph, &c.library);
        assert_eq!(types.get(OutputPort::new(c.p1, 0)), Some(&DataType::Int));

        // Same nodes, binding severed: the passthrough goes polymorphic.
        c.graph.set_input_binding(InputPort::new(c.p1, 0), None);
        types.update(&c.graph, &c.library);

        assert_eq!(
            types.get(OutputPort::new(c.p1, 0)),
            Some(&DataType::Any),
            "the severed input leaves it polymorphic, not `Int` from last time"
        );
        assert_eq!(
            types.get(OutputPort::new(c.p2, 0)),
            Some(&DataType::Any),
            "and the taint reaches downstream"
        );

        // A graph with none of those nodes leaves the table empty rather than
        // answering for ids it no longer contains.
        types.update(&Graph::default(), &c.library);
        assert_eq!(types.get(OutputPort::new(c.p1, 0)), None);
    }

    /// A binding cycle is *present* and `Any` — distinguishable from a port the
    /// table never reached, which is `None`.
    #[test]
    fn a_cycle_resolves_to_any_rather_than_being_absent() {
        let mut c = chain();
        // p1 ← p2 ← p1: the chain now closes on itself.
        c.graph
            .set_input_binding(InputPort::new(c.p1, 0), Binding::bind(c.p2, 0));
        let mut types = OutputTypes::default();

        types.update(&c.graph, &c.library);

        assert_eq!(types.get(OutputPort::new(c.p1, 0)), Some(&DataType::Any));
        assert_eq!(types.get(OutputPort::new(c.p2, 0)), Some(&DataType::Any));
    }
}
