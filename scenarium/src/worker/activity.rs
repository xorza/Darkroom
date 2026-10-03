//! What the worker is doing.

/// What the worker is doing: running a graph, looping its events, both, or neither.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub enum WorkerActivity {
    #[default]
    Idle,
    Executing,
    EventLoop,
    ExecutingEventLoop,
}

impl WorkerActivity {
    pub const fn is_executing(self) -> bool {
        matches!(
            self,
            WorkerActivity::Executing | WorkerActivity::ExecutingEventLoop
        )
    }

    pub const fn event_loop_active(self) -> bool {
        matches!(
            self,
            WorkerActivity::EventLoop | WorkerActivity::ExecutingEventLoop
        )
    }
}
