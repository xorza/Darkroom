use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use common::TempDir;

use tokio::sync::{Notify, mpsc, oneshot};
use tokio::time::{Duration, timeout};

use crate::async_lambda;
use crate::data::const_value::ConstValue;
use crate::data::data_type::DataType;
use crate::data::ram_usage::RamUsage;
use crate::decl::cache_mode::CacheMode;
use crate::decl::func::error::InvokeError;
use crate::decl::func::event_lambda::EventLambda;
use crate::decl::func::func_lambda::{FuncLambda, Invocation};
use crate::exec::cache::disk_store::error::StoreError;
use crate::exec::cache::runtime_cache::error::CacheNodeError;
use crate::exec::program_event::ProgramEvent;
use crate::exec::report::run_reporter::RunPhase;
use crate::identity::NodeId;
use crate::identity::program_id::ProgramId;
use crate::internals::calls::Calls;
use crate::internals::test_graph::TestGraph;
use crate::internals::worker::{PATIENCE, TestWorker};
use crate::worker::Worker;
use crate::worker::activity::WorkerActivity;
use crate::worker::error::{ExitError, WorkerError};
use crate::worker::protocol::{WorkerMessage, WorkerReport};

/// Assert `exit` saw the worker task end in a panic carrying `message`.
fn assert_panicked_with(exited: Result<(), ExitError>, message: &str) {
    let Err(ExitError::Panicked(error)) = exited else {
        panic!("the worker task did not panic: {exited:?}");
    };
    let payload = error.into_panic();
    let text = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str));
    assert_eq!(text, Some(message));
}

mod batching;
mod cache;
mod empty_graph;
mod event_loop;
mod live_progress;
mod replacement;
mod runs;
mod shutdown;
