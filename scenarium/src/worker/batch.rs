use std::sync::Arc;

use indexmap::IndexSet;
use tokio::sync::oneshot;

use crate::execution::RunSeeds;
use crate::execution::compile::CompiledGraph;
use crate::execution::disk_store::DiskStore;
use crate::execution::identity::ExecutionEventPort;
use crate::execution::identity::ExecutionNodeId;
use crate::graph::NodeId;
use crate::worker::protocol::WorkerMessage;

#[derive(Debug)]
pub(crate) enum GraphOp {
    Clear,
    Replace(Arc<CompiledGraph>),
}

#[derive(Debug)]
pub(crate) enum LoopCommand {
    Start,
    Stop,
}

#[derive(Debug, Default)]
pub(crate) struct BatchIntent {
    pub(crate) graph_state: Option<GraphOp>,
    pub(crate) disk_store: Option<DiskStore>,
    pub(crate) loop_request: Option<LoopCommand>,
    pub(crate) execute_sinks: bool,
    pub(crate) execute_event_sources: bool,
    pub(crate) execute_nodes: IndexSet<ExecutionNodeId>,
    pub(crate) evict_cache: IndexSet<NodeId>,
    pub(crate) exit: bool,
    pub(crate) events: IndexSet<ExecutionEventPort>,
    pub(crate) syncs: Vec<oneshot::Sender<()>>,
}

pub(crate) fn scan(msgs: Vec<WorkerMessage>) -> BatchIntent {
    let mut intent = BatchIntent::default();
    for msg in msgs {
        match msg {
            WorkerMessage::Exit => {
                return BatchIntent {
                    exit: true,
                    ..BatchIntent::default()
                };
            }
            WorkerMessage::Update { compiled } => {
                intent.graph_state = Some(GraphOp::Replace(compiled));
            }
            WorkerMessage::Clear => intent.graph_state = Some(GraphOp::Clear),
            WorkerMessage::EvictCache { nodes } => intent.evict_cache.extend(nodes),
            WorkerMessage::SetDiskStore(cache) => intent.disk_store = Some(cache),
            WorkerMessage::Run { seeds } => {
                let RunSeeds {
                    sinks,
                    event_sources,
                    events,
                    nodes,
                } = seeds;
                intent.execute_sinks |= sinks;
                intent.execute_event_sources |= event_sources;
                intent.events.extend(events);
                intent.execute_nodes.extend(nodes);
            }
            WorkerMessage::StartEventLoop => intent.loop_request = Some(LoopCommand::Start),
            WorkerMessage::StopEventLoop => intent.loop_request = Some(LoopCommand::Stop),
            WorkerMessage::Sync { reply } => intent.syncs.push(reply),
        }
    }
    intent
}
