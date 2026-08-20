//! Crate-internal containers with no domain of their own — the shapes the
//! authoring and execution sides both build on, plus the odd operation over a
//! plain one ([`unique`]).

pub(crate) mod column;
pub(crate) mod set;
pub(crate) mod unique;
