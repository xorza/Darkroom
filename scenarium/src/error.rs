use common::DeserializeError;
use thiserror::Error;

use crate::DataType;
use crate::graph::interface::GraphId;
use crate::graph::{InputPort, NodeId, OutputPort};
use crate::node::definition::FuncId;

#[derive(Debug, Error)]
pub enum GraphDeserializeError {
    #[error(transparent)]
    Deserialize(#[from] DeserializeError),
    #[error(transparent)]
    InvalidGraph(#[from] GraphValidationError),
}

#[derive(Debug, Error)]
pub enum GraphValidationError {
    #[error("nested and shared graphs require a subgraph definition")]
    MissingSubgraphDefinition,
    #[error("graph has a nil origin")]
    NilOrigin,
    #[error("graph contains a node with a nil id")]
    NilNodeId,
    #[error("node id {node_id:?} occurs in more than one authoring graph")]
    DuplicateNodeId { node_id: NodeId },
    #[error("node {node_id:?} has a nil func_id")]
    NilFuncId { node_id: NodeId },
    #[error("node {node_id:?} references func {func_id:?}, absent from the library")]
    MissingFunc { node_id: NodeId, func_id: FuncId },
    #[error("node {node_id:?} has a nil graph id")]
    NilGraphId { node_id: NodeId },
    #[error("node {node_id:?} references missing local graph {graph_id:?}")]
    MissingLocalGraph { node_id: NodeId, graph_id: GraphId },
    #[error("node {node_id:?} references a missing graph")]
    MissingGraph { node_id: NodeId },
    #[error("a graph holds at most one GraphInput, found {count}")]
    MultipleGraphInputs { count: usize },
    #[error("a graph holds at most one GraphOutput, found {count}")]
    MultipleGraphOutputs { count: usize },
    #[error("binding on missing node {node_id:?}")]
    BindingMissingNode { node_id: NodeId },
    #[error(
        "input {port_idx} on node {node_id:?} is const-only and cannot be wired to an upstream output",
        node_id = .port.node_id,
        port_idx = .port.port_idx
    )]
    ConstOnlyBinding { port: InputPort },
    #[error(
        "node {destination_id:?} input {port_idx} binds to missing node {source_id:?}",
        destination_id = .destination.node_id,
        port_idx = .destination.port_idx,
        source_id = .producer.node_id
    )]
    BindingMissingProducer {
        destination: InputPort,
        producer: OutputPort,
    },
    #[error(
        "node {node_id:?} input {port_idx} expects {expected:?} but is wired from an incompatible {actual:?}",
        node_id = .destination.node_id,
        port_idx = .destination.port_idx
    )]
    IncompatibleBinding {
        destination: InputPort,
        expected: DataType,
        actual: DataType,
    },
    #[error(
        "node {node_id:?} input {port_idx} holds a constant incompatible with its type {data_type:?}",
        node_id = .port.node_id,
        port_idx = .port.port_idx
    )]
    IncompatibleConstant {
        port: InputPort,
        data_type: DataType,
    },
    #[error("subscription from missing emitter {node_id:?}")]
    MissingSubscriptionEmitter { node_id: NodeId },
    #[error("node {emitter:?} event {event_idx} has missing subscriber {subscriber:?}")]
    MissingSubscriber {
        emitter: NodeId,
        event_idx: usize,
        subscriber: NodeId,
    },
    #[error("pinned output on missing node {node_id:?}")]
    PinnedOutputMissingNode { node_id: NodeId },
    #[error("exposed event {name:?} names missing emitter {emitter:?}")]
    ExposedEventMissingEmitter { name: String, emitter: NodeId },
    #[error("exposed event index {event_idx} out of range on {emitter:?}")]
    ExposedEventOutOfRange { emitter: NodeId, event_idx: usize },
    #[error("local graph has a nil id")]
    NilLocalGraphId,
    #[error("graph {name:?} is recursive (contains itself)")]
    RecursiveGraph { name: String },
    #[error("in local graph {name:?}: {source}")]
    LocalGraph {
        name: String,
        #[source]
        source: Box<GraphValidationError>,
    },
    #[error("in shared graph {name:?}: {source}")]
    SharedGraph {
        name: String,
        #[source]
        source: Box<GraphValidationError>,
    },
    #[error("entry graph cannot have a subgraph definition")]
    EntryDefinition,
    #[error("entry graph cannot contain interface boundary nodes")]
    EntryBoundaryNodes,
}
