# Issues

- `scenarium/src/execution/compile/consumer_cone`: the module's public
  documentation links to two private items, `RuntimeCache::evict` and
  `ConsumerCone`. `cargo doc -p scenarium` warns on both.
