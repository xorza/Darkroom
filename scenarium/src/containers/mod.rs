//! Crate-internal containers with no domain of their own — the shapes the
//! authoring and execution sides both build on, plus the odd operation over a
//! plain one ([`unique`]).

pub(crate) mod col;
pub(crate) mod idx_set;
pub(crate) mod unique;
