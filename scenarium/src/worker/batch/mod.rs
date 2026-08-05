use std::sync::Arc;

use tokio::sync::oneshot;

use crate::common::unique;
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

/// One wake's messages reduced to what the worker will actually do, one slot
/// per kind — so a burst of edits installs once rather than N times.
///
/// **The graph op is the batch's frame of reference.** Reducing to slots loses
/// the order the messages arrived in, so the order they are *applied* in is a
/// standing rule rather than a caller's choice, and
/// [`WorkerTask::apply_intent`](crate::worker::task::WorkerTask) states it:
/// `graph_state` lands first, and every stage after it — the store, the cache
/// maintenance, the run — is about the program it left installed. Anything
/// added here that reads "the installed program" belongs after it too, or it
/// will silently act on the one the batch is replacing.
#[derive(Debug, Default)]
pub(crate) struct BatchIntent {
    pub(crate) graph_state: Option<GraphOp>,
    pub(crate) disk_store: Option<DiskStore>,
    /// Whether this batch owes every installed node a blob — see
    /// [`WorkerMessage::FlushAllCaches`].
    pub(crate) flush_all_caches: bool,
    pub(crate) loop_request: Option<LoopCommand>,
    /// What this batch's `Run` messages ask for, coalesced — plus the events
    /// the running loop fired, which arrive outside any message.
    ///
    /// The seeds themselves rather than a second spelling of their four
    /// fields: combining is [`RunSeeds::merge`]'s, and what leaves here is the
    /// same value the engine is handed, so nothing has to be taken apart and
    /// put back together across the worker boundary.
    pub(crate) seeds: RunSeeds,
    pub(crate) evict_cache: Vec<NodeId>,
    pub(crate) flush_cache: Vec<NodeId>,
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
                WorkerMessage::EvictCache { nodes } => unique::extend(&mut self.evict_cache, nodes),
                WorkerMessage::FlushCache { nodes } => unique::extend(&mut self.flush_cache, nodes),
                WorkerMessage::FlushAllCaches => self.flush_all_caches = true,
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
        self.flush_all_caches = false;
        self.loop_request = None;
        self.seeds.clear();
        self.evict_cache.clear();
        self.flush_cache.clear();
        self.syncs.clear();
    }
}

#[cfg(test)]
mod tests;
