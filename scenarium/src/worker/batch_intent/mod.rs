use std::sync::Arc;

use crate::containers::unique;
use crate::exec::cache::disk_store::DiskStore;
use crate::exec::compile::compiled_graph::CompiledGraph;
use crate::exec::program_event::ProgramEvent;
use crate::exec::seeds::RunSeeds;
use crate::identity::program_id::ProgramId;
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
/// **The graph op is the batch's frame of reference.** Reducing to slots loses arrival order, so
/// the apply order is a standing rule, stated by
/// [`WorkerTask::apply_intent`](crate::worker::worker_task::WorkerTask): `graph_state` lands
/// first, and every later stage (store, cache maintenance, run) is about the program it left
/// installed. Anything added here that reads "the installed program" belongs after it too.
#[derive(Debug, Default)]
pub(crate) struct BatchIntent {
    pub(crate) graph_state: Option<GraphOp>,
    pub(crate) disk_store: Option<DiskStore>,
    pub(crate) ram_headroom: Option<u64>,
    /// Whether this batch owes every installed node a blob — see
    /// [`WorkerMessage::FlushAllCaches`].
    pub(crate) flush_all_caches: bool,
    pub(crate) loop_request: Option<LoopCommand>,
    /// What this batch's run messages ask for, coalesced, plus the events the running loop fired,
    /// which arrive outside any message.
    pub(crate) seeds: RunSeeds,
    pub(crate) evict_cache: Vec<ProgramId>,
    pub(crate) flush_cache: Vec<ProgramId>,
    /// What waits for this batch to commit, as a test's `Sync` does.
    #[cfg(test)]
    #[expect(
        clippy::absolute_paths,
        reason = "a test-only field names its type in place of a cfg'd import"
    )]
    pub(crate) syncs: Vec<tokio::sync::oneshot::Sender<()>>,
}

impl BatchIntent {
    /// Merge `msg` into the batch, after the messages merged before it.
    pub(crate) fn push(&mut self, msg: WorkerMessage) {
        match msg {
            // A graph op supersedes the node seeds named before it, which asked about the program
            // it replaces, so one its program lacks is void. A seed named after the last graph op
            // still fails the run when absent. The events fired before it go whole: each names
            // the program the loop it came from was armed over.
            WorkerMessage::Update { compiled } => {
                self.seeds
                    .program_ids
                    .retain(|&program_id| compiled.contains(program_id));
                self.seeds.events.clear();
                self.graph_state = Some(GraphOp::Replace(compiled));
            }
            WorkerMessage::Clear => {
                self.seeds.program_ids.clear();
                self.seeds.events.clear();
                self.graph_state = Some(GraphOp::Clear);
            }
            WorkerMessage::EvictCache { program_ids } => {
                unique::extend(&mut self.evict_cache, program_ids);
            }
            WorkerMessage::FlushCache { program_ids } => {
                unique::extend(&mut self.flush_cache, program_ids);
            }
            WorkerMessage::FlushAllCaches => self.flush_all_caches = true,
            WorkerMessage::SetDiskStore(cache) => self.disk_store = Some(cache),
            WorkerMessage::SetRamHeadroom(bytes) => self.ram_headroom = Some(bytes),
            WorkerMessage::RunSinks => self.seeds.sinks = true,
            WorkerMessage::RunNodes { program_ids } => {
                unique::extend(&mut self.seeds.program_ids, program_ids);
            }
            #[cfg(test)]
            WorkerMessage::FireEvents { events } => {
                for event in events {
                    self.push_event(event);
                }
            }
            WorkerMessage::StartEventLoop => self.loop_request = Some(LoopCommand::Start),
            WorkerMessage::StopEventLoop => self.loop_request = Some(LoopCommand::Stop),
            #[cfg(test)]
            WorkerMessage::Sync { reply } => self.syncs.push(reply),
        }
    }

    /// Merge an event the running loop fired, once however often it fires.
    ///
    /// **A fired event lives no longer than the loop that fired it**, which armed it over the
    /// program then installed: a graph op merged after it voids it ([`push`](Self::push)), a
    /// batch that stops or rebuilds the loop voids its own ([`void_events`](Self::void_events)),
    /// and once that loop's tasks are joined, so none fires again, the worker voids what they
    /// left in the mailbox. So every event the planner reads names the installed program.
    pub(crate) fn push_event(&mut self, event: ProgramEvent) {
        unique::extend(&mut self.seeds.events, [event]);
    }

    /// Forget the fired events, as the loop that fired them stops.
    pub(crate) fn void_events(&mut self) {
        self.seeds.events.clear();
    }

    /// Forget every run the batch asks for, as a cancel does. Everything else stays: a graph op,
    /// a store, a headroom, cache maintenance and a loop command are not runs.
    pub(crate) fn void_runs(&mut self) {
        self.seeds.clear();
    }

    /// Whether the batch asks the worker for nothing.
    pub(crate) fn is_empty(&self) -> bool {
        let Self {
            graph_state,
            disk_store,
            ram_headroom,
            flush_all_caches,
            loop_request,
            seeds,
            evict_cache,
            flush_cache,
            #[cfg(test)]
            syncs,
        } = self;
        let empty = graph_state.is_none()
            && disk_store.is_none()
            && ram_headroom.is_none()
            && !flush_all_caches
            && loop_request.is_none()
            && seeds.is_empty()
            && evict_cache.is_empty()
            && flush_cache.is_empty();
        #[cfg(test)]
        let empty = empty && syncs.is_empty();
        empty
    }

    /// Forget the batch, keeping the buffers.
    pub(crate) fn clear(&mut self) {
        self.graph_state = None;
        self.disk_store = None;
        self.ram_headroom = None;
        self.flush_all_caches = false;
        self.loop_request = None;
        self.seeds.clear();
        self.evict_cache.clear();
        self.flush_cache.clear();
        #[cfg(test)]
        self.syncs.clear();
    }
}

#[cfg(test)]
mod tests;
