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

impl BatchIntent {
    pub(crate) fn reset(&mut self, msgs: impl IntoIterator<Item = WorkerMessage>) {
        self.clear();
        for msg in msgs {
            match msg {
                WorkerMessage::Exit => {
                    self.clear();
                    self.exit = true;
                    return;
                }
                WorkerMessage::Update { compiled } => {
                    self.graph_state = Some(GraphOp::Replace(compiled));
                }
                WorkerMessage::Clear => self.graph_state = Some(GraphOp::Clear),
                WorkerMessage::EvictCache { nodes } => self.evict_cache.extend(nodes),
                WorkerMessage::SetDiskStore(cache) => self.disk_store = Some(cache),
                WorkerMessage::Run { seeds } => {
                    let RunSeeds {
                        sinks,
                        event_sources,
                        events,
                        nodes,
                    } = seeds;
                    self.execute_sinks |= sinks;
                    self.execute_event_sources |= event_sources;
                    self.events.extend(events);
                    self.execute_nodes.extend(nodes);
                }
                WorkerMessage::StartEventLoop => self.loop_request = Some(LoopCommand::Start),
                WorkerMessage::StopEventLoop => self.loop_request = Some(LoopCommand::Stop),
                WorkerMessage::Sync { reply } => self.syncs.push(reply),
            }
        }
    }

    fn clear(&mut self) {
        self.graph_state = None;
        self.disk_store = None;
        self.loop_request = None;
        self.execute_sinks = false;
        self.execute_event_sources = false;
        self.execute_nodes.clear();
        self.evict_cache.clear();
        self.exit = false;
        self.events.clear();
        self.syncs.clear();
    }
}
