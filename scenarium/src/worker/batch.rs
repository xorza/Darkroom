use std::collections::HashSet;
use std::hash::Hash;
use std::sync::Arc;

use tokio::sync::oneshot;

use crate::execution::RunSeeds;
use crate::execution::compile::CompiledGraph;
use crate::execution::disk_store::DiskStore;
use crate::execution::event::EventRef;
use crate::execution::identity::NodeAddress;
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
    pub(crate) execute_event_triggers: bool,
    pub(crate) execute_nodes: OrderedUnique<NodeAddress>,
    pub(crate) exit: bool,
    pub(crate) events: OrderedUnique<EventRef>,
    pub(crate) syncs: Vec<oneshot::Sender<()>>,
}

#[derive(Debug)]
pub(crate) struct OrderedUnique<T> {
    pub(crate) values: Vec<T>,
    pub(crate) seen: HashSet<T>,
}

impl<T> Default for OrderedUnique<T> {
    fn default() -> Self {
        Self {
            values: Vec::new(),
            seen: HashSet::new(),
        }
    }
}

impl<T: Clone + Eq + Hash> OrderedUnique<T> {
    pub(crate) fn extend(&mut self, values: impl IntoIterator<Item = T>) {
        for value in values {
            if self.seen.insert(value.clone()) {
                self.values.push(value);
            }
        }
    }

    pub(crate) fn take(&mut self) -> Vec<T> {
        self.seen.clear();
        std::mem::take(&mut self.values)
    }
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
            WorkerMessage::InjectEvents { events } => intent.events.extend(events),
            WorkerMessage::Update { compiled } => {
                intent.graph_state = Some(GraphOp::Replace(compiled));
            }
            WorkerMessage::Clear => intent.graph_state = Some(GraphOp::Clear),
            WorkerMessage::SetDiskStore(cache) => intent.disk_store = Some(cache),
            WorkerMessage::Run { seeds } => {
                let RunSeeds {
                    sinks,
                    event_triggers,
                    events,
                    nodes,
                } = seeds;
                intent.execute_sinks |= sinks;
                intent.execute_event_triggers |= event_triggers;
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
