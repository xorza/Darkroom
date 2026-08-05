# Issues

- darkroom: `PendingTransition::OpenAt` is never constructed and
  `open_document_at` is never called — two dead-code warnings.

- palantir: `profiling::finish_frame!()` fires once per `Window::frame`,
  which is per-window. With more than one window open, Tracy's main frame
  marker ticks once per window per host loop iteration, so reported frame
  times are per-window slices rather than real frames.
