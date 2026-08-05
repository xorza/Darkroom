//! Resolved output types for authoring graphs, filled in whole rather than
//! asked one port at a time.

use hashbrown::HashMap;

use crate::graph::Graph;
use crate::graph::identity::OutputPort;
use crate::library::Library;
use crate::{ConstValue, DataType};

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
        value: ConstValue,
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
mod tests;
