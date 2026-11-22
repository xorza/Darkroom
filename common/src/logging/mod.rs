//! The process's one logger: [`Logger`](logger::Logger) takes `tracing` events and `log` records
//! alike, filters them by `RUST_LOG`'s directives, writes each as a line, and keeps the warnings
//! and errors in a [`LogRing`](log_ring::LogRing) for a caller to report. Refused:
//! `tracing-subscriber` with `tracing-log`, seven crates and with them `regex`'s three, for a
//! level filter, a line format and a ring, when nothing in the workspace opens a span.

mod log_filter;
mod log_line;
pub(super) mod log_ring;
pub(super) mod logger;
