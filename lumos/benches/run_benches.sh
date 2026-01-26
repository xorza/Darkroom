#!/bin/bash

OUTPUT_FILE="benchmark_results.txt"

BENCHES=(
    "demosaic_bayer"
    "demosaic_xtrans"
    "hot_pixels"
    "math"
    "median_filter"
    "registration"
    "stack_mean"
    "stack_median"
    "stack_sigma_clipped"
    "star_detection_background"
    "star_detection_centroid"
    "star_detection_convolution"
    "star_detection_cosmic_ray"
    "star_detection_deblend"
    "star_detection_detection"
)

# Compile all benchmarks first
echo "Compiling all benchmarks..."
for bench in "${BENCHES[@]}"; do
    echo "  Compiling $bench..."
    cargo bench -p lumos --features bench --bench "$bench" --no-run
done

# Clear/create output file
> "$OUTPUT_FILE"

# Run all benchmarks
echo "Running benchmarks..."
for bench in "${BENCHES[@]}"; do
    echo "  Running $bench..."
    echo "=== $bench ===" >> "$OUTPUT_FILE"
    cargo bench -p lumos --features bench --bench "$bench" 2>&1 >> "$OUTPUT_FILE"
    echo "" >> "$OUTPUT_FILE"
done

echo "Done! Results saved to $OUTPUT_FILE"
