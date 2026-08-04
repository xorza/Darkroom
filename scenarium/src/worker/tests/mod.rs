use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use ::common::TempDir;

use tokio::sync::{Notify, mpsc, oneshot};
use tokio::time::{Duration, timeout};

use crate::execution::cache::disk_store::error::StoreError;
use crate::execution::cache::runtime::error::CacheNodeError;
use crate::execution::error::Error;
use crate::execution::report::NodeExecutionStatus;
use crate::execution::seeds::RunSeeds;
use crate::graph::func::error::InvokeError;
use crate::graph::func::event::EventLambda;
use crate::graph::func::lambda::{FuncLambda, Invocation};
use crate::graph::identity::{EventPort, NodeId};
use crate::graph::node::CacheMode;
use crate::testing::calls::Calls;
use crate::testing::graph::TestGraph;
use crate::testing::worker::TestWorker;
use crate::worker::Worker;
use crate::worker::error::WorkerError;
use crate::worker::protocol::{WorkerMessage, WorkerReport};
use crate::worker::status::{WorkerActivity, WorkerStatusKind};
use crate::{ConstValue, DataType, RamUsage, async_lambda};

/// How long a "nothing happens" claim watches for before it is believed.
const QUIET: Duration = Duration::from_millis(100);

mod batching;
mod cache;
mod empty_graph;
mod event_loop;
mod live_progress;
mod replacement;
mod runs;
mod shutdown;
