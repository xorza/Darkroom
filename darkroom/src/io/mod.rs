//! On-disk I/O and session state: file-dialog [`persistence`] for
//! documents/themes/subgraphs, and the [`config`] that remembers the
//! last theme + document across launches.

pub mod config;
pub mod persistence;
