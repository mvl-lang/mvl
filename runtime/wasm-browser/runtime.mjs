// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Schuberg Philis
//
// JS host shim for the MVL `wasm-browser` target (#2093 Phase 2, ADR-0063).
//
// Every MVL-compiled WASM module unconditionally imports
// `wasi_snapshot_preview1.fd_write`/`clock_time_get` (println + the
// allocator's heap-init read), whether or not the program uses
// std.random/std.time/std.env/std.io. A plain browser has no WASI host, so
// this file supplies the two WASI functions actually used, backed by
// `console.log`/`console.error`/`Date.now()` — the "take what the host
// already offers" primitives, no bespoke protocol invented.
//
// `mvl_runtime_wasm.wasm` (built for `wasm32-unknown-unknown`, see
// `runtime/wasm/src/lib.rs` and `make build-runtime-wasm-browser`) supplies
// everything `std.random`/`std.time` need — this file only has to bridge
// its one remaining host dependency, `env._mvl_js_now_ms` (`Date.now()`).
//
// Usage:
//
//   import { instantiateMvlProgram } from "./runtime.mjs";
//   const { programInstance } = await instantiateMvlProgram(
//     await fetch("mvl_runtime_wasm.wasm").then(r => r.arrayBuffer()),
//     await fetch("my_program.wasm").then(r => r.arrayBuffer()),
//   );
//   programInstance.exports._start(); // WASI entry point convention

/** `Date.now()` in ms — the browser runtime module's one host dependency. */
export function createEnvShim() {
  return {
    _mvl_js_now_ms: () => Date.now(),
  };
}

/**
 * WASI shim for the two functions every MVL WASM module imports.
 * `getMemory` is called lazily, on first use — at import-binding time (before
 * `WebAssembly.instantiate` returns) the program's own `memory` export
 * doesn't exist yet, but by the time these functions are actually called
 * (during program execution) it does.
 */
export function createWasiShim(getMemory) {
  const decoder = new TextDecoder("utf-8");
  // println/eprintln each make TWO fd_write calls — one for the message,
  // one for a separate "\n" (see wasm_text.rs's WASI runtime blob) — so a
  // single call's text can't be assumed to be a whole line. Line-buffer
  // per fd instead: accumulate and flush to console.log/error only once a
  // "\n" has actually arrived, splitting on it rather than guessing from
  // one call's shape.
  const pending = new Map(); // fd -> buffered text without a newline yet

  return {
    // fd_write(fd, iovs_ptr, iovs_len, nwritten_ptr) -> errno.
    // iovs is an array of { buf: u32, buf_len: u32 } (8 bytes each). Only
    // fd 1 (stdout) and 2 (stderr) are meaningful here — println/eprintln
    // are the only WASM-backend callers — anything else is accepted and
    // silently dropped rather than trapping, matching WASI's "no fd 0/3+
    // file access" story on this target.
    fd_write(fd, iovsPtr, iovsLen, nwrittenPtr) {
      const buffer = getMemory().buffer;
      const view = new DataView(buffer);
      let text = "";
      let total = 0;
      for (let i = 0; i < iovsLen; i++) {
        const base = iovsPtr + i * 8;
        const ptr = view.getUint32(base, true);
        const len = view.getUint32(base + 4, true);
        text += decoder.decode(new Uint8Array(buffer, ptr, len));
        total += len;
      }
      view.setUint32(nwrittenPtr, total, true);
      const sink = fd === 1 ? console.log : fd === 2 ? console.error : null;
      if (sink) {
        const buffered = (pending.get(fd) ?? "") + text;
        const lines = buffered.split("\n");
        pending.set(fd, lines.pop()); // last element has no trailing "\n" yet
        lines.forEach((line) => sink(line));
      }
      return 0; // __WASI_ERRNO_SUCCESS
    },

    // clock_time_get(clock_id, precision, time_ptr) -> errno. Writes a u64
    // nanosecond timestamp — `Date.now()` (ms) is the finest granularity a
    // browser offers here, widened to nanoseconds so the layout matches
    // what a real WASI host would write.
    clock_time_get(_clockId, _precision, timePtr) {
      const buffer = getMemory().buffer;
      const nanos = BigInt(Date.now()) * 1_000_000n;
      new DataView(buffer).setBigUint64(timePtr, nanos, true);
      return 0; // __WASI_ERRNO_SUCCESS
    },
  };
}

/**
 * Instantiate the browser-targeted `mvl_runtime_wasm.wasm` and an
 * `mvl build --backend=wasm` program together, wiring the program's
 * `(import "runtime" ...)` declarations straight to the runtime module's
 * exports and its `(import "wasi_snapshot_preview1" ...)` declarations to
 * the shim above.
 *
 * @param {BufferSource} runtimeBytes - `mvl_runtime_wasm.wasm` (browser build)
 * @param {BufferSource} programBytes - the compiled MVL program
 * @returns {Promise<{runtimeInstance: WebAssembly.Instance, programInstance: WebAssembly.Instance}>}
 */
export async function instantiateMvlProgram(runtimeBytes, programBytes) {
  const { instance: runtimeInstance } = await WebAssembly.instantiate(runtimeBytes, {
    env: createEnvShim(),
  });

  let programMemory;
  const { instance: programInstance } = await WebAssembly.instantiate(programBytes, {
    wasi_snapshot_preview1: createWasiShim(() => programMemory),
    runtime: runtimeInstance.exports,
  });
  programMemory = programInstance.exports.memory;

  return { runtimeInstance, programInstance };
}
