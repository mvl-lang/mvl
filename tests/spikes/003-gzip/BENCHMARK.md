# gzip spike — performance log

Benchmark: `make benchmark ITERS=1000` (512-byte "Hello World!" repeated payload)

## Results

| Pass | µs/iter | Cumulative | Change |
|------|---------|------------|--------|
| Baseline | 3181 | 1x | Initial implementation: bit-at-a-time BitWriter, for-loop LZ77 with full 258-iter match_length, bytes-to-bits DEFLATE decoder, wrong CRC32 |
| Early-exit match + direct bit reads | 616 | 5.2x | `match_length` while-loop exits on first mismatch; DEFLATE decoder reads bits directly from byte array (eliminates 8x `List[Int]` expansion); `while !done` replaces for+flag |
| Best-match early exit | 500 | 6.4x | `find_best_match` stops scanning when match reaches max possible length |
| Greedy + bulk writes + while-loop | 391 | 8.1x | Nearest-first scan + stop at first match >= 3; `write_bits` accumulates in wide buffer then flushes bytes (inspired by miniz_oxide `put_bits`); `while pos < n` replaces `for _ in range(0, n)` eliminating 498 no-op `enc_step` calls |
| `mvl build --release` | **194** | **16.4x** | Added `--release` flag to `mvl build`/`mvl run`. Enables Rust compiler optimizations: inlining, bounds-check elision, LLVM opt passes. Same MVL code, no algorithmic changes. |

## Reference implementations

| Implementation | µs/iter | vs MVL release | Notes |
|----------------|---------|----------------|-------|
| system gzip (C) | ~6700 | 34x slower | Process spawn overhead dominates |
| rust/flate2 (release) | ~20 | 10x faster | In-process, hash-based LZ77, SIMD match |
| mvl/gzip (debug) | ~414 | 2.1x slower | Same code, unoptimized Rust (`cargo build`) |
| **mvl/gzip (release)** | **~194** | **baseline** | Same code, optimized Rust (`cargo build --release`) |

## Approaches tested but rejected

| Approach | Result | Reason |
|----------|--------|--------|
| Hash-based LZ77 (`List::filled(256)` + `List.set`) | 412 µs (5% slower) | Per-call table allocation (~20µs) exceeds lookup savings for 512B payload. Would win at 4KB+ where O(n×window) linear scan dominates. |
| CRC32 lookup table (`List::filled(256)` + precompute) | 395 µs (within noise) | Table build (256 × 8 iters) + allocation offsets saving vs on-the-fly (512 × 8 iters). No net improvement at 512B. |
| Inline `find_best_match` (ref vars, no struct) | 403 µs (within noise) | Rust optimizes small struct copies well even in debug. Eliminating `SearchState` + `update_search` had no measurable impact. |

## Conclusion

**194µs/iter in release mode — 16.4x faster than initial implementation, within 10x of flate2.**

The optimization journey had two distinct phases:

1. **Algorithmic (3181µs → 391µs, 8.1x)**: Early-exit loops, direct bit reads, greedy matching, bulk bit writes, while-loop LZ77. All changes to MVL source code.

2. **Compiler (391µs → 194µs, 2.0x)**: `mvl build --release` passes `--release` to `cargo build`. Rust compiler eliminates bounds checks, inlines functions, runs LLVM optimization passes. Zero changes to MVL source.

The remaining 10x gap to flate2 is dominated by:
- 8-byte SIMD match comparison (flate2 uses `u64 XOR` + `trailing_zeros`; MVL does 1 byte/iter)
- Pre-allocated flat arrays (flate2 uses `&mut [u8]`; MVL uses `Vec::push()`)
- Hash-based LZ77 (flate2: 1 hash lookup/position; MVL: linear scan — blocked by `List::filled` allocation cost at 512B)

## How to run

```bash
# Quick (10 iterations)
make -C tests/spikes/003-gzip benchmark

# Stable numbers (1000 iterations)
make -C tests/spikes/003-gzip benchmark ITERS=1000

# Unit tests
make -C tests/spikes/003-gzip test

# Run with release optimizations directly
mvl run --release tests/spikes/003-gzip/gzip_perf.mvl -- --iterations 100
```
