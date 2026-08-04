# Issues

- Opening a document writes the *previous* document's resident disk-backed
  values into the newly opened document's `.darkroom-cache/`: the worker applies
  a batch's `SetDiskStore` (and the `flush_all_caches` sweep it triggers) before
  the batch's graph op, so the sweep runs against the outgoing program while the
  store already points at the incoming document's root.
