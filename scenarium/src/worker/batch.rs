use std::sync::Arc;

use indexmap::IndexSet;
use tokio::sync::oneshot;

use crate::execution::cache::disk_store::DiskStore;
use crate::execution::compile::compiled_graph::CompiledGraph;
use crate::execution::seeds::RunSeeds;
use crate::graph::identity::{EventPort, NodeId};
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
    /// What this batch's `Run` messages ask for, coalesced — plus the events
    /// the running loop fired, which arrive outside any message.
    ///
    /// The seeds themselves rather than a second spelling of their four
    /// fields: combining is [`RunSeeds::merge`]'s, and what leaves here is the
    /// same value the engine is handed, so nothing has to be taken apart and
    /// put back together across the worker boundary.
    pub(crate) seeds: RunSeeds,
    pub(crate) evict_cache: IndexSet<NodeId>,
    pub(crate) syncs: Vec<oneshot::Sender<()>>,
}

impl BatchIntent {
    pub(crate) fn reset(
        &mut self,
        msgs: impl IntoIterator<Item = WorkerMessage>,
        events: impl IntoIterator<Item = EventPort>,
    ) {
        self.clear();
        for msg in msgs {
            match msg {
                WorkerMessage::Update { compiled } => {
                    self.graph_state = Some(GraphOp::Replace(compiled));
                }
                WorkerMessage::Clear => self.graph_state = Some(GraphOp::Clear),
                WorkerMessage::EvictCache { nodes } => self.evict_cache.extend(nodes),
                WorkerMessage::SetDiskStore(cache) => self.disk_store = Some(cache),
                WorkerMessage::Run { seeds } => self.seeds.merge(seeds),
                WorkerMessage::StartEventLoop => self.loop_request = Some(LoopCommand::Start),
                WorkerMessage::StopEventLoop => self.loop_request = Some(LoopCommand::Stop),
                WorkerMessage::Sync { reply } => self.syncs.push(reply),
            }
        }
        self.seeds.add_events(events);
    }

    fn clear(&mut self) {
        self.graph_state = None;
        self.disk_store = None;
        self.loop_request = None;
        self.seeds.clear();
        self.evict_cache.clear();
        self.syncs.clear();
    }
}
