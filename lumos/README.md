# Lumos

Astrophotography image stacking library in Rust.

## Features

### Image Stacking
- **Sigma clipping** with GPU acceleration for outlier rejection
- **Local normalization** using tile-based statistics (128x128 tiles) for handling varying sky backgrounds
- **Live stacking** with real-time preview during imaging sessions
- **Multi-session integration** with per-session quality weighting

### Registration
- **Phase correlation** for sub-pixel alignment
- **Star detection** with hybrid GPU/CPU approach
- **Distortion correction**:
  - Radial distortion (barrel/pincushion) via Brown-Conrady model
  - Tangential distortion correction
  - Field curvature (Petzval) correction

### Astrometry
- **Plate solving** with quad geometric hashing
- **WCS (World Coordinate System)** computation
- **Gaia DR3 catalog** integration via VizieR API

### Specialized Stacking Modes
- **Comet/asteroid stacking** with dual-stack approach for moving objects
- **Session-weighted stacking** combining multiple imaging sessions
- **Gradient removal** with polynomial and RBF models

## Usage

```rust
use lumos::prelude::*;

// Load frames
let frames: Vec<AstroImage> = load_frames("./lights/*.fits");

// Configure stacking
let config = StackConfig::default()
    .with_rejection(RejectionMethod::SigmaClip { sigma: 2.5 })
    .with_normalization(NormalizationMethod::LocalNormalization);

// Stack images
let result = stack_images(&frames, &config)?;
```

### Live Stacking

```rust
use lumos::prelude::*;

let config = LiveStackConfig {
    mode: LiveStackMode::WeightedMean,
    normalize: true,
    ..Default::default()
};

let mut accumulator = LiveStackAccumulator::new(width, height, channels, config);

// Add frames as they arrive
for frame in incoming_frames {
    let quality = compute_frame_quality(&frame);
    let stats = accumulator.add_frame(&frame, quality);
    
    // Get preview for display
    let preview = accumulator.preview();
}

let final_result = accumulator.finalize();
```

### Multi-Session Stacking

```rust
use lumos::prelude::*;

let config = SessionConfig::default();
let mut multi_session = MultiSessionStack::new(config);

// Add sessions from different nights
multi_session.add_session(session1);
multi_session.add_session(session2);

// Stack with automatic quality weighting
let result = multi_session.stack()?;
```

## GPU Acceleration

GPU acceleration is available for:
- Sigma clipping (5-18% faster for >50 frames)
- Star detection threshold mask generation
- Mask dilation with shared memory tiling

Requires wgpu-compatible GPU. Falls back to CPU when unavailable.

## Building

```bash
cargo build -p lumos --release
```

## Running Tests

```bash
cargo nextest run -p lumos
```

## Benchmarks

```bash
cargo test -p lumos --release <bench_name> -- --ignored --nocapture
```

Example:
```bash
cargo test -p lumos --release bench_extract_candidates -- --ignored --nocapture
```

## License

See repository root for license information.
