# Issues

- `palantir` `Slider` never reports `committed` for a click that does not
  latch a drag: `show` reads only `response.left.drag.stopped()`, so a press
  and release on the track writes the value with no commit edge behind it.
  `DragValue` may have the same gap.
- `palantir` image registry queues never drain in a deviceless harness: a CPU
  recorder that registers images keeps every `Image`'s bytes in `pending` and
  every dropped id in `dropped` for the life of the harness.
