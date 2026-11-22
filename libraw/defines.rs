/// One list for both the compiled library and the parsed headers, so the two cannot disagree:
/// `build.rs` compiles LibRaw with it and `bindgen/` parses the headers under it. Static only, so
/// `LIBRAW_NODLL` everywhere. Thread-safe (no `LIBRAW_NOTHREADS`): frames decode on concurrent
/// rayon workers, each with its own `libraw_data_t`. `USE_ZLIB` for deflate DNG, as LibRaw's own
/// `Makefile.dist` builds it, against the zlib `libz-sys` builds.
const DEFINES: &[&str] = &["LIBRAW_NODLL", "USE_ZLIB"];
