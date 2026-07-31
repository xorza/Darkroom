use std::sync::Arc;

use super::*;
use crate::execution::compile::error::CompileError;
use crate::execution::error::{Error, RunError};
use crate::execution::seeds::RunSeeds;
use crate::graph::func::FuncBehavior;
use crate::graph::func::error::InvokeError;
use crate::graph::func::lambda::{Invocation, OutputDemand, internals};
use crate::graph::identity::{NodeId, OutputPort};
use crate::graph::node::CacheMode;
use crate::library::Library;
use crate::testing::TestFuncHooks;
use crate::testing::engine::{RunOutcome, TestEngine};
use crate::testing::graph::{NodeSpec, TestGraph};
use crate::{DataType, DynamicValue, StaticValue};
use ::common::{CancelToken, FloatExt};
use tokio::sync::Mutex;

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

mod argument_values;
mod behavior;
mod cache_persistence;
mod compile_regressions;
mod const_bindings;
mod disabled_nodes;
mod error_propagation;
mod events;
mod execution;
mod graph_structure;
mod installation;
mod mid_run_release;
mod missing_inputs;
mod node_seeds;
mod output_demand;
mod resource_binds;
mod stats;
mod topology;
