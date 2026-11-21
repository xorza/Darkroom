//! [`PortsError`]: why a list of ports is no declaration's, and the [`PortList`] it names.

use std::fmt;

use thiserror::Error;

use crate::identity::PortId;

/// Which side of a declaration a port lies on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortList {
    Inputs,
    Outputs,
    Events,
}

impl fmt::Display for PortList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Inputs => "input",
            Self::Outputs => "output",
            Self::Events => "event",
        })
    }
}

/// Why a list of ports is no declaration's: what [`Ports::check`](crate::Ports::check) refuses.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum PortsError {
    #[error("{list} {idx} repeats the id {port_id:?} of an earlier one")]
    DuplicateId {
        list: PortList,
        idx: usize,
        port_id: PortId,
    },
    #[error("{list} {idx} names a type under the nil id")]
    NilType { list: PortList, idx: usize },
    #[error("input {idx} declares a default that matches neither its type nor its domain")]
    InvalidDefault { idx: usize },
    #[error("input {idx} declares a span of another number type than its own")]
    DomainMismatch { idx: usize },
}

/// Why [`Graph::admit`](crate::Graph::admit) refuses a binding.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum BindingRejection {
    #[error("its node does not declare the input")]
    UndeclaredInput,
    #[error("the producer does not declare the output")]
    UndeclaredOutput,
    #[error("the input is const-only and cannot take a wire")]
    WiredConstOnly,
    #[error("the input does not accept the constant")]
    ConstMismatch,
    #[error("the input does not accept the producer's output type")]
    WireMismatch,
}
