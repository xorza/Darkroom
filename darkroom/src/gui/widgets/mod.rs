//! Reusable, domain-agnostic UI widgets composed from palantir
//! primitives. Unlike the `node`/`canvas` modules these hold no graph
//! knowledge — a caller maps the widget's returned event onto its own
//! intent.
pub mod inline_rename;
