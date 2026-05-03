//! Rust implementations of `std.random` stdlib functions.
//!
//! Non-deterministic PRNG backed by xorshift64, seeded from `SystemTime` on
//! first use. NOT cryptographically secure — use `std.crypto.crypto_random_bytes`
//! for that. Re-exported via `mvl_runtime::prelude::*`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

// ── PRNG state ─────────────────────────────────────────────────────────────

static PRNG_STATE: OnceLock<AtomicU64> = OnceLock::new();

fn prng_state() -> &'static AtomicU64 {
    PRNG_STATE.get_or_init(|| {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64 ^ (d.as_secs().wrapping_mul(0x9e37_79b9_7f4a_7c15)))
            .unwrap_or(0xdeadbeef_cafebabe);
        // xorshift64 must not start with 0.
        AtomicU64::new(if seed == 0 { 1 } else { seed })
    })
}

/// xorshift64 — one step, returns the new state as the output value.
fn xorshift64(state: u64) -> u64 {
    let mut x = state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    x
}

fn next_u64() -> u64 {
    let state = prng_state();
    // CAS loop to make concurrent callers safe without a mutex.
    loop {
        let old = state.load(Ordering::Relaxed);
        let new = xorshift64(old);
        if state
            .compare_exchange_weak(old, new, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return new;
        }
    }
}

// ── Public stdlib functions ────────────────────────────────────────────────

/// Returns a random integer in `[min, max]` (inclusive). Requires `! Random`.
pub fn int(min: i64, max: i64) -> i64 {
    if min >= max {
        return min;
    }
    let range = (max - min) as u64 + 1;
    let r = next_u64() % range;
    min + r as i64
}

/// Returns a random float in `[0.0, 1.0)`. Requires `! Random`.
pub fn float() -> f64 {
    // Shift top 53 bits into the mantissa of an f64 in [1.0, 2.0), then subtract 1.
    let bits = (next_u64() >> 11) | 0x3FF0_0000_0000_0000u64;
    f64::from_bits(bits) - 1.0
}

/// Returns `n` pseudo-random bytes as a `Vec<i64>` in `[0, 255]`. Requires `! Random`.
pub fn bytes(n: i64) -> Vec<i64> {
    let count = n.max(0) as usize;
    let mut out = Vec::with_capacity(count);
    let mut i = 0;
    while i < count {
        let word = next_u64();
        for shift in (0..8u32).take(count - i) {
            out.push(((word >> (shift * 8)) & 0xFF) as i64);
        }
        i += 8;
    }
    out.truncate(count);
    out
}

/// Returns a random element from the list, or `None` if empty. Requires `! Random`.
pub fn choice<T: Clone>(list: Vec<T>) -> Option<T> {
    if list.is_empty() {
        return None;
    }
    let idx = (next_u64() as usize) % list.len();
    Some(list[idx].clone())
}

/// Shuffles the list using Fisher-Yates and returns it. Requires `! Random`.
pub fn shuffle<T>(mut list: Vec<T>) -> Vec<T> {
    let n = list.len();
    for i in (1..n).rev() {
        let j = (next_u64() as usize) % (i + 1);
        list.swap(i, j);
    }
    list
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_returns_value_in_range() {
        for _ in 0..1000 {
            let v = int(1, 6);
            assert!((1..=6).contains(&v), "int(1,6) out of range: {v}");
        }
    }

    #[test]
    fn int_min_equals_max_returns_min() {
        assert_eq!(int(42, 42), 42);
    }

    #[test]
    fn int_min_greater_than_max_returns_min() {
        assert_eq!(int(10, 5), 10);
    }

    #[test]
    fn float_in_unit_interval() {
        for _ in 0..1000 {
            let v = float();
            assert!((0.0..1.0).contains(&v), "float() out of [0,1): {v}");
        }
    }

    #[test]
    fn bytes_returns_correct_length() {
        assert_eq!(bytes(0).len(), 0);
        assert_eq!(bytes(1).len(), 1);
        assert_eq!(bytes(7).len(), 7);
        assert_eq!(bytes(16).len(), 16);
    }

    #[test]
    fn bytes_values_in_byte_range() {
        for b in bytes(64) {
            assert!((0..=255).contains(&b), "byte out of range: {b}");
        }
    }

    #[test]
    fn bytes_negative_returns_empty() {
        assert!(bytes(-5).is_empty());
    }

    #[test]
    fn choice_empty_returns_none() {
        let empty: Vec<i64> = vec![];
        assert!(choice(empty).is_none());
    }

    #[test]
    fn choice_single_returns_element() {
        assert_eq!(choice(vec![99i64]), Some(99));
    }

    #[test]
    fn choice_returns_element_from_list() {
        let list = vec![10i64, 20, 30];
        for _ in 0..100 {
            let v = choice(list.clone()).unwrap();
            assert!(list.contains(&v));
        }
    }

    #[test]
    fn shuffle_empty_is_noop() {
        let v: Vec<i64> = vec![];
        assert!(shuffle(v).is_empty());
    }

    #[test]
    fn shuffle_single_is_noop() {
        assert_eq!(shuffle(vec![42i64]), vec![42]);
    }

    #[test]
    fn shuffle_preserves_elements() {
        let original = vec![1i64, 2, 3, 4, 5];
        let mut shuffled = shuffle(original.clone());
        shuffled.sort();
        let mut expected = original;
        expected.sort();
        assert_eq!(shuffled, expected);
    }

    #[test]
    fn xorshift64_not_zero() {
        // xorshift64 must never produce 0 from a non-zero seed.
        let mut state = 1u64;
        for _ in 0..10_000 {
            state = xorshift64(state);
            assert_ne!(state, 0);
        }
    }

    #[test]
    fn consecutive_calls_differ() {
        // With overwhelming probability, two consecutive draws differ.
        let a = next_u64();
        let b = next_u64();
        assert_ne!(
            a, b,
            "two consecutive next_u64() calls returned the same value"
        );
    }
}
