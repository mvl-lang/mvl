# gzip spike — performance log

Benchmark: `make benchmark ITERS=1000` (512-byte "Hello World!" repeated payload)

## Results

| Pass | µs/iter | Cumulative | Change |
|------|---------|------------|--------|
| Baseline | 3181 | 1x | Initial implementation: bit-at-a-time BitWriter, for-loop LZ77 with full 258-iter match_length, bytes-to-bits DEFLATE decoder, wrong CRC32 |
| Early-exit match + direct bit reads | 616 | 5.2x | `match_length` while-loop exits on first mismatch; DEFLATE decoder reads bits directly from byte array (eliminates 8x `List[Int]` expansion); `while !done` replaces for+flag |
| Best-match early exit | 500 | 6.4x | `find_best_match` stops scanning when match reaches max possible length |
| Greedy + bulk writes + while-loop | **391** | **8.1x** | Nearest-first scan + stop at first match >= 3; `write_bits` accumulates in wide buffer then flushes bytes (inspired by miniz_oxide `put_bits`); `while pos < n` replaces `for _ in range(0, n)` eliminating 498 no-op `enc_step` calls |

## Reference implementations

| Implementation | µs/iter | vs MVL | Notes |
|----------------|---------|--------|-------|
| system gzip (C) | ~6500 | 0.06x | Process spawn overhead dominates — MVL is 16x faster |
| rust/flate2 (release) | ~12 | 33x faster | In-process, optimized, hash-based LZ77, SIMD match |
| **mvl/gzip (debug)** | **391** | **baseline** | Pure MVL, compiled to unoptimized Rust |
| mvl/gzip (release) | ~325 | 1.2x faster | Same code, `cargo build --release` |

## Approaches tested but rejected

| Approach | Result | Reason |
|----------|--------|--------|
| Hash-based LZ77 (`List::filled(256)` + `List.set`) | 412 µs (5% slower) | Per-call table allocation (~20µs) exceeds lookup savings for 512B payload. Would win at 4KB+ where O(n×window) linear scan dominates. |
| CRC32 lookup table (`List::filled(256)` + precompute) | 395 µs (within noise) | Table build (256 × 8 iters) + allocation offsets saving vs on-the-fly (512 × 8 iters). No net improvement at 512B. |
| Inline `find_best_match` (ref vars, no struct) | 403 µs (within noise) | Rust optimizes small struct copies well even in debug. Eliminating `SearchState` + `update_search` had no measurable impact. |

## Conclusion

**391µs is the algorithmic floor for 512B payloads with the current MVL runtime.** All remaining approaches that avoid per-call allocation converge to ~390-405µs. The gap to flate2 (~12µs, ~33x) is dominated by runtime/compiler factors, not algorithmic choices.

## Remaining gap analysis (391µs vs flate2's 12µs ≈ 33x)

**Algorithmic — tested, no further gains at 512B:**

| Approach | Tested | Result |
|----------|--------|--------|
| CRC32 lookup table | Yes | Within noise — allocation cost offsets savings |
| Inline struct threading | Yes | Within noise — Rust optimizes struct copies |
| Hash-based LZ77 | Yes | Slower — allocation dominates at 512B |

**Runtime/compiler (not fixable in MVL code):**

**Runtime/compiler (not fixable in MVL code):**

| Factor | MVL | flate2 | Gap |
|--------|-----|--------|-----|
| Build profile | debug (bounds checks, no inlining) | release (full optimization) | ~1.2x |
| Match comparison | 1 byte/iter via `List.get().unwrap_or().to_int()` | 8 bytes/iter via u64 XOR + trailing_zeros | ~8x |
| Output buffer | `Vec::push()` with reallocation | Pre-allocated `&mut [u8]` with direct indexing | ~1.5x |
| Hash table | Linear scan (no `List.set` at time of testing) | Flat `u16[]` array, O(1) lookup | ~2x (for larger payloads) |

## How to run

```bash
# Quick (10 iterations)
make -C tests/spikes/003-gzip benchmark

# Stable numbers (1000 iterations)
make -C tests/spikes/003-gzip benchmark ITERS=1000

# Unit tests
make -C tests/spikes/003-gzip test
```
