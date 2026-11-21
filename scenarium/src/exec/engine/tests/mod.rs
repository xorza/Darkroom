use std::sync::Arc;

use super::*;
use crate::data::const_value::ConstValue;
use crate::data::data_type::DataType;
use crate::data::dynamic_value::DynamicValue;
use crate::exec::error::{ExecError, RunError};
use crate::exec::idx::NodeIdx;
use crate::exec::seeds::RunSeeds;
use crate::graph::func::FuncBehavior;
use crate::graph::func::error::InvokeError;
use crate::graph::func::lambda::{Invocation, OutputDemand};
use crate::graph::identity::NodeId;
use crate::graph::node::CacheMode;
use crate::graph::program_id::ProgramId;
use crate::internals::calls::Calls;
use crate::internals::engine::{ReportedRun, RunOutcome, TestEngine};
use crate::internals::test_graph::{NodeSpec, TestGraph};
use common::CancelToken;

use crate::async_lambda;
use crate::internals::blob::{BLOB_TYPE, Blob};

mod argument_values;
mod behavior;
mod cache_persistence;
mod compile_regressions;
mod const_bindings;
mod disabled_nodes;
mod error_propagation;
mod events;
mod exec;
mod graph_structure;
mod installation;
mod mid_run_release;
mod missing_inputs;
mod node_seeds;
mod output_demand;
mod resource_binds;
mod topology;
mod twins;

/// The bytes of a node's first input, a [`Blob`], or none.
pub(super) fn blob(inputs: &[DynamicValue]) -> Vec<u8> {
    inputs
        .first()
        .and_then(|value| value.as_custom::<Blob>())
        .map_or_else(Vec::new, |blob| blob.0.clone())
}

/// A pure node kept as `cache` says that outputs its blob input, or none, with 64 bytes more.
pub(super) fn grows(cache: CacheMode) -> impl FnOnce(NodeSpec) -> NodeSpec {
    move |n| {
        n.pure()
            .optional(DataType::Custom(BLOB_TYPE))
            .output(DataType::Custom(BLOB_TYPE))
            .cache(cache)
            .lambda(async_lambda!(move |Invocation {
                                            inputs, outputs, ..
                                        }| {
                let mut bytes = blob(inputs);
                bytes.extend([0; 64]);
                outputs[0] = DynamicValue::Custom(Arc::new(Blob(bytes)));
                Ok(())
            }))
    }
}
