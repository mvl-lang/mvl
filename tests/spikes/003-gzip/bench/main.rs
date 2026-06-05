/// Rust/flate2 gzip compress+decompress benchmark.
/// Usage: gzip-bench <iterations> <payload-file>
///
/// Reads the payload once, then runs compress+decompress N times,
/// printing total wall-clock seconds.
use flate2::read::{GzDecoder, GzEncoder};
use flate2::Compression;
use std::io::Read;
use std::time::Instant;
use std::{env, fs, process};

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: gzip-bench <iterations> <payload-file>");
        process::exit(1);
    }
    let iterations: usize = args[1].parse().unwrap_or_else(|_| {
        eprintln!("bad iteration count");
        process::exit(1);
    });
    let payload = fs::read(&args[2]).unwrap_or_else(|e| {
        eprintln!("cannot read {}: {e}", args[2]);
        process::exit(1);
    });

    // Warm up
    let _ = roundtrip(&payload);

    let start = Instant::now();
    for _ in 0..iterations {
        let decoded = roundtrip(&payload);
        assert_eq!(decoded.len(), payload.len());
    }
    let elapsed = start.elapsed();
    let secs = elapsed.as_secs_f64();
    let per_iter_us = (secs / iterations as f64) * 1_000_000.0;

    println!(
        "rust/flate2: {} iterations, {:.3}s total, {:.1}µs/iter, {:.1} MB/s",
        iterations,
        secs,
        per_iter_us,
        (payload.len() as f64 * iterations as f64) / secs / 1_000_000.0,
    );
}

fn roundtrip(data: &[u8]) -> Vec<u8> {
    // Compress
    let mut encoder = GzEncoder::new(data, Compression::fast());
    let mut compressed = Vec::new();
    encoder.read_to_end(&mut compressed).unwrap();

    // Decompress
    let mut decoder = GzDecoder::new(compressed.as_slice());
    let mut decompressed = Vec::new();
    decoder.read_to_end(&mut decompressed).unwrap();
    decompressed
}
