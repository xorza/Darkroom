//! Wire ties on the canvas: where each tie's band stands and where each of its strands passes
//! ([`TieLayout`](tie_layout::TieLayout)), and the band painted over them ([`band`]).
//! `darkroom/AGENTS.md`, "Wire ties", holds the decisions.

pub(crate) mod band;
pub(crate) mod tie_layout;

#[cfg(test)]
mod tests;
