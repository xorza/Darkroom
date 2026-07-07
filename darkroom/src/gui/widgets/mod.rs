//! Reusable, domain-agnostic UI widgets composed from aperture
//! primitives. Unlike the `node`/`canvas` modules these hold no graph
//! knowledge — a caller maps the widget's returned event onto its own
//! intent.
pub(crate) mod inline_rename;
