//! On-disk I/O and session state: file-dialog [`persistence`] for
//! documents/themes/subgraphs, the [`config`] that remembers the last theme +
//! document across launches, and the per-document disk-[`cache`] location.

pub mod cache;
pub mod config;
pub mod library;
pub mod persistence;
