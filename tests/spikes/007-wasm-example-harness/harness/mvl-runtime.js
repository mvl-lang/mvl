// mvl-runtime.js — standalone JS implementation of the MVL WASM runtime.
//
// Ported verbatim (mechanical TS -> JS translation, no logic changes) from
// mvl-lang/mvl-playground's web/src/runtime/mvl-runtime.ts, so this harness
// tests the actual runtime the playground ships, not a reimplementation of
// it. If mvl-runtime.ts changes, port the change here too — divergence
// between this file and the playground's is exactly the kind of drift this
// whole session has been about catching.
//
// The MVL WASM backend (mvl build --backend=wasm) emits imports under a
// "runtime" namespace: a shared WebAssembly.Memory plus ~60 functions for
// string/array/option/result/map operations. The WASM module uses a bump
// allocator within this memory, writes string bytes, and passes (ptr, len)
// pairs to runtime functions. Functions that "create" values (string_new,
// array_new, option_some, etc.) return opaque i32 handles into
// runtime-managed tables.
//
// Source: mvl-lang/mvl-playground web/src/runtime/mvl-runtime.ts,
// mvl-lang/mvl release v1.7.2 runtime.

let nextHandle = 1;

function newHandle() {
  return nextHandle++;
}

// Bump allocator for structs, carving real linear-memory addresses.
//
// Found while building experiment 010 (mastermind_web) and reverse-
// engineering score_guess's WAT: `_mvl_struct_alloc`'s return value is NOT
// treated as an opaque JS-side handle by the compiled module — the WASM
// code does raw `i64.store offset=N` / `i64.load offset=N` directly on it
// (e.g. score_guess writes Feedback.blacks/whites at +0/+8 of whatever
// _mvl_struct_alloc returned; render_feedback reads a Feedback struct back
// the same way). A JS-side handle-table index (1, 2, 3, ...) used as a raw
// memory address corrupts real module memory — those low addresses overlap
// static rodata (string literals, etc.) in any nontrivial module.
//
// This previously used the same handle-table pattern as every other
// _mvl_*_alloc/new function here (fine for those — arrays/strings/options
// are only ever read back through a runtime FUNCTION CALL, e.g.
// _mvl_option_value_i64, never via raw i64.load on the handle itself).
// Structs are the one exception: verified directly via an isolated
// score_guess probe that a real bump allocator is required, a handle
// table silently produces garbage.
//
// This candidate root-causing actor_trading's crash (mvl-lang/mvl#2083,
// filed before this fix existed): actor_trading's main.wat calls
// $_mvl_struct_alloc 18 times (Order/Fill are both structs), so this
// harness had been running that test against a broken allocator the whole
// time the "actor message routing" bug was diagnosed and filed. Re-run
// after this fix, below, to check whether the crash is actually this bug
// instead.
//
// Starts at 1MB — far above any static data a realistically-sized example
// module would emit — and grows the shared memory on demand. Never frees;
// fine for a short-lived test-harness process, not fine for a long-running
// host (documented in README).
const SCRATCH_HEAP_BASE = 32768;
const PAGE_SIZE = 65536;

export function createMvlRuntime() {
  const memory = new WebAssembly.Memory({ initial: 1, maximum: 256 });
  const u8 = () => new Uint8Array(memory.buffer);

  let scratchHeapPtr = SCRATCH_HEAP_BASE;
  function bumpAllocScratch(size) {
    const end = scratchHeapPtr + size;
    const neededPages = Math.ceil(end / PAGE_SIZE);
    const currentPages = memory.buffer.byteLength / PAGE_SIZE;
    if (neededPages > currentPages) {
      memory.grow(neededPages - currentPages);
    }
    const ptr = scratchHeapPtr;
    scratchHeapPtr = (end + 7) & ~7; // 8-byte align, matches typical wasm ABI expectations
    return ptr;
  }

  const arrays = new Map();
  const options = new Map();
  const results = new Map();
  const maps = new Map();

  const decoder = new TextDecoder("utf-8");
  const encoder = new TextEncoder();

  function readString(ptr, len) {
    if (len <= 0) return "";
    return decoder.decode(u8().subarray(ptr, ptr + len));
  }

  // String-CREATING functions (_mvl_string_new/clone/concat/substring/
  // to_upper/to_lower/trim/replace) hit the exact same bug as
  // _mvl_struct_alloc/_mvl_array_get (see comment above bumpAllocScratch):
  // their return value is read back via raw `i32.load offset=0`
  // (data ptr) / `i32.load offset=4` (byte len) directly on the compiled
  // module side, e.g. `s.concat(s)` compiles to
  //   call $_mvl_string_concat
  //   local.tee $tmp / i32.load offset=0 / ... / i32.load offset=4
  // — an 8-byte {ptr:i32, len:i32} descriptor in real linear memory, NOT a
  // JS-side string handle. storeString/strings-map (as still used by every
  // OTHER experiment 008 test case that never exercised .concat()-style
  // chains) silently produced garbage here for the same reason structs did.
  // Fixed the same way: write the UTF-8 bytes into scratch memory, write an
  // 8-byte descriptor pointing at them, return the descriptor's address.
  function storeString(s) {
    const bytes = encoder.encode(s);
    const dataPtr = bumpAllocScratch(bytes.length);
    if (bytes.length > 0) u8().set(bytes, dataPtr);
    const descPtr = bumpAllocScratch(8);
    const dv = new DataView(memory.buffer);
    dv.setInt32(descPtr, dataPtr, true);
    dv.setInt32(descPtr + 4, bytes.length, true);
    return descPtr;
  }

  // Read back a string previously created by storeString, given its
  // descriptor pointer (NOT a JS handle — see storeString above).
  function readStoredString(descPtr) {
    const dv = new DataView(memory.buffer);
    const dataPtr = dv.getInt32(descPtr, true);
    const len = dv.getInt32(descPtr + 4, true);
    return readString(dataPtr, len);
  }

  function storeArray(a) {
    const h = newHandle();
    arrays.set(h, a);
    return h;
  }

  function storeOption(tag, value) {
    const h = newHandle();
    options.set(h, { tag, value });
    return h;
  }

  function storeResult(tag, value, error) {
    const h = newHandle();
    results.set(h, { tag, value, error });
    return h;
  }

  function storeMap(m) {
    const h = newHandle();
    maps.set(h, m);
    return h;
  }

  const runtime = {
    memory,

    // ── String functions (ptr, len) -> handle or scalar ──────────────

    _mvl_string_eq: (p1, l1, p2, l2) =>
      readString(p1, l1) === readString(p2, l2) ? 1 : 0,

    _mvl_string_len: (ptr, len) => BigInt([...readString(ptr, len)].length),

    _mvl_string_is_empty: (ptr, len) => (readString(ptr, len).length === 0 ? 1 : 0),

    _mvl_string_contains: (p1, l1, p2, l2) =>
      readString(p1, l1).includes(readString(p2, l2)) ? 1 : 0,

    _mvl_string_starts_with: (p1, l1, p2, l2) =>
      readString(p1, l1).startsWith(readString(p2, l2)) ? 1 : 0,

    _mvl_string_ends_with: (p1, l1, p2, l2) =>
      readString(p1, l1).endsWith(readString(p2, l2)) ? 1 : 0,

    _mvl_string_find: (p1, l1, p2, l2) => {
      const idx = readString(p1, l1).indexOf(readString(p2, l2));
      return BigInt(idx);
    },

    _mvl_string_new: (ptr, len) => storeString(readString(ptr, len)),

    // No confirmed call site (String has no .clone() method reachable from
    // MVL source as of this compiler version — verified: `s.clone()`
    // produces "no method `clone` on type `String`"). Kept for import
    // completeness, consistent with the descriptor convention in case a
    // future compiler version emits it. Takes a descriptor pointer, not a
    // JS handle.
    _mvl_string_clone: (descPtr) => storeString(readStoredString(descPtr)),

    // Never frees (see bumpAllocScratch) — fine for a short-lived harness
    // process, not a real GC. h is a descriptor pointer now, not a handle;
    // nothing to look up or delete.
    _mvl_string_drop: (_descPtr) => {},

    _mvl_string_concat: (p1, l1, p2, l2) =>
      storeString(readString(p1, l1) + readString(p2, l2)),

    _mvl_string_substring: (ptr, len, start, end) => {
      const s = readString(ptr, len);
      const chars = [...s];
      const lo = Math.max(0, Number(start));
      const hi = Math.max(0, Number(end));
      return storeString(chars.slice(lo, Math.min(hi, chars.length)).join(""));
    },

    _mvl_string_to_upper: (ptr, len) => storeString(readString(ptr, len).toUpperCase()),

    _mvl_string_to_lower: (ptr, len) => storeString(readString(ptr, len).toLowerCase()),

    _mvl_string_trim: (ptr, len) => storeString(readString(ptr, len).trim()),

    _mvl_string_replace: (sp, sl, fp, fl, tp, tl) =>
      storeString(readString(sp, sl).split(readString(fp, fl)).join(readString(tp, tl))),

    _mvl_string_parse_int: (ptr, len) => {
      const s = readString(ptr, len).trim();
      const n = Number(s);
      if (s !== "" && !isNaN(n) && Number.isInteger(n)) {
        return storeResult(0, BigInt(n));
      }
      return storeResult(1, 0n, `invalid integer: "${s}"`);
    },

    // ── Array functions (handle-based) ────────────────────────────────

    _mvl_array_new: (_elemType, _capacity) => storeArray([]),

    _mvl_array_len: (h) => BigInt(arrays.get(h)?.length ?? 0),

    _mvl_array_is_empty: (h) => ((arrays.get(h)?.length ?? 1) === 0 ? 1 : 0),

    _mvl_array_push: (h, val) => {
      arrays.get(h)?.push(val);
    },

    _mvl_array_push_i32: (h, val) => {
      arrays.get(h)?.push(val);
    },

    _mvl_array_push_i64: (h, val) => {
      arrays.get(h)?.push(val);
    },

    _mvl_array_push_f64: (h, val) => {
      arrays.get(h)?.push(val);
    },

    // Same raw-pointer convention as _mvl_struct_alloc (see top-of-file
    // comment): the WAT for a `for x in [literal, array]` loop does
    // `call $_mvl_array_get / i64.load offset=0` on the return value, so
    // this must return a real memory address holding the element, not the
    // element itself. Contrast with _mvl_array_get_option_i64/i32 below,
    // which ARE read back through a runtime function call
    // (_mvl_option_value_i64/i32) and so are safe as JS-side handles.
    // Writes at the width matching how the element was pushed (i64 as
    // BigInt, i32/f64 as Number) so whichever *.load the caller uses reads
    // the right bytes.
    _mvl_array_get: (h, idx) => {
      const arr = arrays.get(h);
      const i = Number(idx);
      const val = arr && i >= 0 && i < arr.length ? arr[i] : 0;
      const ptr = bumpAllocScratch(8);
      const dv = new DataView(memory.buffer);
      if (typeof val === "bigint") dv.setBigInt64(ptr, val, true);
      else if (Number.isInteger(val)) dv.setInt32(ptr, val, true);
      else dv.setFloat64(ptr, val, true);
      return ptr;
    },

    _mvl_array_clone: (h) => {
      const arr = arrays.get(h);
      return storeArray(arr ? [...arr] : []);
    },

    _mvl_array_drop: (h) => {
      arrays.delete(h);
    },

    _mvl_string_ptr_array_drop: (h) => {
      arrays.delete(h);
    },

    _mvl_string_ptr_array_dedup: (h) => {
      const arr = arrays.get(h);
      if (arr) {
        const seen = new Set();
        const deduped = arr.filter((v) => {
          if (seen.has(v)) return false;
          seen.add(v);
          return true;
        });
        arrays.set(h, deduped);
      }
    },

    _mvl_array_dedup_i64: (h) => {
      const arr = arrays.get(h);
      if (arr) {
        const seen = new Set();
        arrays.set(h, arr.filter((v) => (seen.has(v) ? false : (seen.add(v), true))));
      }
    },

    _mvl_array_dedup_i32: (h) => {
      const arr = arrays.get(h);
      if (arr) {
        const seen = new Set();
        arrays.set(h, arr.filter((v) => (seen.has(v) ? false : (seen.add(v), true))));
      }
    },

    _mvl_array_contains_i64: (h, val) => {
      const arr = arrays.get(h);
      return arr && arr.includes(val) ? 1 : 0;
    },

    _mvl_array_contains_i32: (h, val) => {
      const arr = arrays.get(h);
      return arr && arr.includes(val) ? 1 : 0;
    },

    _mvl_array_insert_i64: (h, val) => {
      arrays.get(h)?.push(val);
    },

    _mvl_array_insert_i32: (h, val) => {
      arrays.get(h)?.push(val);
    },

    // ── Option functions ─────────────────────────────────────────────
    //
    // Tag convention: 0 = Some (has a value), 1 = None. Verified empirically
    // against the compiler's actual WAT output (not assumed): a minimal
    // `xs.get(i).unwrap_or(dflt)` compiles to
    //   call $_mvl_option_tag / i32.eqz / if (result i64)   <- Some branch
    //     call $_mvl_option_value_i64
    //   else                                                 <- None branch
    //     local.get $dflt
    // i32.eqz branches on tag==0, and that branch reads the value — so
    // tag==0 must mean Some. This was previously inverted here (0=None,
    // 1=Some, matching mvl-playground's runtime.ts) which silently returned
    // wrong values (not a crash) for every unwrap_or/Option-consuming call.
    // See experiments/008_mvl_example_wasm_harness/README.md for the fix
    // writeup and probe script.

    _mvl_option_some_i64: (val) => storeOption(0, val),
    _mvl_option_some_i32: (val) => storeOption(0, val),
    _mvl_option_none: () => storeOption(1, 0),

    _mvl_option_tag: (h) => options.get(h)?.tag ?? 1,

    _mvl_option_value_i64: (h) => {
      const opt = options.get(h);
      return opt ? BigInt(opt.value) : 0n;
    },

    _mvl_option_value_i32: (h) => {
      const opt = options.get(h);
      return opt ? Number(opt.value) : 0;
    },

    _mvl_option_drop: (h) => {
      options.delete(h);
    },

    _mvl_array_get_option_i64: (h, idx) => {
      const arr = arrays.get(h);
      if (!arr) return storeOption(1, 0);
      const i = Number(idx);
      if (i < 0 || i >= arr.length) return storeOption(1, 0);
      return storeOption(0, BigInt(arr[i]));
    },

    _mvl_array_get_option_i32: (h, idx) => {
      const arr = arrays.get(h);
      if (!arr) return storeOption(1, 0);
      const i = Number(idx);
      if (i < 0 || i >= arr.length) return storeOption(1, 0);
      return storeOption(0, arr[i]);
    },

    // ── Result functions ──────────────────────────────────────────────
    //
    // Same tag inversion fix as Option, verified the same way (a minimal
    // `s.parse_int().unwrap_or(dflt)` probe): 0 = Ok, 1 = Err.

    _mvl_result_ok_i64: (val) => storeResult(0, val),
    _mvl_result_ok_i32: (val) => storeResult(0, val),
    _mvl_result_err_str: (ptr, len) => storeResult(1, 0, readString(ptr, len)),

    _mvl_result_tag: (h) => results.get(h)?.tag ?? 1,

    _mvl_result_value_i64: (h) => {
      const r = results.get(h);
      return r ? BigInt(r.value) : 0n;
    },

    _mvl_result_value_i32: (h) => {
      const r = results.get(h);
      return r ? Number(r.value) : 0;
    },

    _mvl_result_drop: (h) => {
      results.delete(h);
    },

    // ── Map functions ─────────────────────────────────────────────────

    _mvl_map_new_si64: () => storeMap(new Map()),

    _mvl_map_len: (h) => BigInt(maps.get(h)?.size ?? 0),

    _mvl_map_insert_si64: (h, kp, kl, val) => {
      maps.get(h)?.set(readString(kp, kl), val);
    },

    _mvl_map_get_si64: (h, kp, kl) => {
      const m = maps.get(h);
      if (!m) return storeOption(1, 0);
      const val = m.get(readString(kp, kl));
      if (val === undefined) return storeOption(1, 0);
      return storeOption(0, BigInt(val));
    },

    _mvl_map_contains_key_si64: (h, kp, kl) =>
      maps.get(h)?.has(readString(kp, kl)) ? 1 : 0,

    _mvl_map_drop_si64: (h) => {
      maps.delete(h);
    },

    // ── Struct ─────────────────────────────────────────────────────────

    // Real bump allocator into linear memory — see the top-of-file comment.
    // NOT a handle-table index like every other _mvl_*_alloc/new here.
    _mvl_struct_alloc: (size) => bumpAllocScratch(size),

    // ── Audit (no-op — this harness doesn't persist an audit trail) ───

    _mvl_audit_emit_relabel: (
      _tagPtr, _tagLen,
      _fromPtr, _fromLen,
      _toPtr, _toLen,
      _filePtr, _fileLen,
      _line, _col,
    ) => {
      // No-op, matching mvl-playground's own runtime.
    },
  };

  return { memory, runtime };
}
