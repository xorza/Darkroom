//! [`PortRef`]: one data port's identity in the graph.

use scenarium::{InputPort, NodeId, OutputPort};

use crate::gui::node_key::NodeKey;

/// Whether a data port consumes a binding (`Input`) or produces a value
/// (`Output`); an event is addressed by the graph's own [`EventPort`](scenarium::EventPort).
/// `Input` ports live in the left column, `Output` in the right; `opposite`
/// flips between them for snap-target tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum PortKind {
    Input,
    Output,
}

impl PortKind {
    pub(crate) const fn opposite(self) -> Self {
        match self {
            Self::Input => Self::Output,
            Self::Output => Self::Input,
        }
    }
}

/// One data port as the canvas addresses it: an input and an output in one
/// key, so a wire drag, a hover and a geometry table can hold either end.
/// Domain-keyed so UI passes derive its `WidgetId` (see
/// `crate::gui::pane::graph_ui::node_ui::port_row::port_circle_wid`) without
/// threading a cache. Node ids are unique across the whole document, so no
/// graph ref is needed alongside — enforced by
/// [`Document::validate`](crate::core::document::Document::validate).
///
/// An edit names the graph's own [`InputPort`] or [`OutputPort`], reached
/// through [`Self::input_port`] and [`Self::output_port`], which check the kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PortRef {
    pub(crate) node_id: NodeId,
    pub(crate) kind: PortKind,
    pub(crate) port_idx: usize,
}

impl NodeKey for PortRef {
    fn node(self) -> NodeId {
        self.node_id
    }
}

impl PortRef {
    /// `node_id`'s `port_idx`th input — the left-column counterpart of the
    /// graph's [`InputPort`].
    pub(crate) const fn input(node_id: NodeId, port_idx: usize) -> Self {
        Self {
            node_id,
            kind: PortKind::Input,
            port_idx,
        }
    }

    /// `node_id`'s `port_idx`th output — the right-column counterpart of the
    /// graph's [`OutputPort`].
    pub(crate) const fn output(node_id: NodeId, port_idx: usize) -> Self {
        Self {
            node_id,
            kind: PortKind::Output,
            port_idx,
        }
    }

    /// The graph's input this names, or `None` when it names an output.
    pub(crate) const fn input_port(self) -> Option<InputPort> {
        match self.kind {
            PortKind::Input => Some(InputPort::new(self.node_id, self.port_idx)),
            PortKind::Output => None,
        }
    }

    /// The graph's output this names, or `None` when it names an input.
    pub(crate) const fn output_port(self) -> Option<OutputPort> {
        match self.kind {
            PortKind::Output => Some(OutputPort::new(self.node_id, self.port_idx)),
            PortKind::Input => None,
        }
    }
}

impl From<InputPort> for PortRef {
    fn from(port: InputPort) -> Self {
        Self::input(port.node_id, port.port_idx)
    }
}

impl From<OutputPort> for PortRef {
    fn from(port: OutputPort) -> Self {
        Self::output(port.node_id, port.port_idx)
    }
}

#[cfg(test)]
mod tests {
    use scenarium::{InputPort, NodeId, OutputPort};

    use crate::gui::port_ref::PortRef;

    /// Each conversion answers only for its own kind, and a graph port survives
    /// the trip through a `PortRef` unchanged.
    #[test]
    fn a_port_converts_only_to_its_own_kind() {
        let node = NodeId::unique();
        let input = InputPort::new(node, 3);
        let output = OutputPort::new(node, 3);

        assert_eq!(PortRef::from(input).input_port(), Some(input));
        assert_eq!(PortRef::from(input).output_port(), None);
        assert_eq!(PortRef::from(output).output_port(), Some(output));
        assert_eq!(PortRef::from(output).input_port(), None);
    }
}
