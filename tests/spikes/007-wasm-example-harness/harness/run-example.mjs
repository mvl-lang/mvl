// run-example.mjs — compile an MVL source file to WASM, execute it against
// this repo's ported mvl-runtime.js + WASI shim, and capture stdout/stderr.
//
// This is the actual gap this experiment exists to close: mvl-playground is
// currently the ONLY host anywhere that can execute `mvl build --backend=wasm`
// output. `mvl run` doesn't accept --backend=wasm; bare `wasmtime run` fails
// with `unknown import: runtime::memory has not been defined`, because the
// runtime namespace is a playground-specific convention, not a standard WASI
// world. This script is a second, independent host implementing that same
// convention, so "does this MVL program actually run under the WASM backend"
// becomes testable outside a browser.
//
// Usage: node run-example.mjs <path/to/main.mvl>
import { execFileSync } from "node:child_process";
import { readFileSync, existsSync } from "node:fs";
import { dirname, join, basename } from "node:path";
import { WASI, File, OpenFile, ConsoleStdout } from "@bjorn3/browser_wasi_shim";
import { createMvlRuntime } from "./mvl-runtime.js";

const mvlFile = process.argv[2];
if (!mvlFile) {
  console.error("Usage: node run-example.mjs <path/to/main.mvl>");
  process.exit(2);
}

const dir = dirname(mvlFile);
const entry = basename(mvlFile);
const watPath = join(dir, entry.replace(/\.mvl$/, ".wat"));
const wasmPath = join(dir, entry.replace(/\.mvl$/, ".wasm"));

// 1. Compile: mvl build --backend=wasm (produces WAT text, not a binary).
// "WAT written to: ..." etc. are the compiler's own build-status messages,
// not the program's output — piped and re-emitted on *this process's
// stderr*, not stdout, so a caller capturing this script's stdout gets
// exactly what the compiled program printed, nothing else. Still visible
// (not discarded), so a compile failure is still diagnosable.
const buildOutput = execFileSync("mvl", ["build", entry, "--backend=wasm"], {
  cwd: dir,
  stdio: ["inherit", "pipe", "inherit"],
});
process.stderr.write(buildOutput);

if (!existsSync(watPath)) {
  console.error(`expected ${watPath} after mvl build --backend=wasm, not found`);
  process.exit(1);
}

// 2. WAT -> WASM binary. mvl-playground's own backend does this same
// conversion server-side via the Rust `wat` crate (`wat::parse_bytes`);
// `wasm-tools parse` is the CLI-equivalent conversion, not a different path.
execFileSync("wasm-tools", ["parse", watPath, "-o", wasmPath]);
const bytes = readFileSync(wasmPath);

// 3. Instantiate against BOTH import namespaces the compiled module needs:
// the custom "runtime" (the ~60-function convention) and WASI (for stdout/
// stderr via println!/eprintln!). Both share one WebAssembly.Memory — the
// module imports "runtime.memory" and re-exports it as "memory", which is
// exactly what lets the WASI shim's own internal `instance.exports.memory`
// access work against the same bytes the runtime functions read/write.
const stdout = [];
const stderr = [];
const wasi = new WASI(
  [entry],
  [],
  [
    new OpenFile(new File([])),
    ConsoleStdout.lineBuffered((msg) => stdout.push(msg)),
    ConsoleStdout.lineBuffered((msg) => stderr.push(msg)),
  ],
);

const { runtime } = createMvlRuntime();
const { instance } = await WebAssembly.instantiate(bytes, {
  runtime,
  wasi_snapshot_preview1: wasi.wasiImport,
});

let exitCode = 0;
let crashed = false;
try {
  exitCode = wasi.start(instance);
} catch (err) {
  // The full stack trace (not just err.message) is what actually locates a
  // WASM-level trap: V8 includes the wasm call frames (function names,
  // module offsets) leading to the fault, e.g.
  //   at order_book_submit (wasm://.../wasm-function[66]:0xbbe)
  //   at order_book_dispatch (...)
  //   at __mvl_actor_route (...)
  // A message-only catch would have reported just "memory access out of
  // bounds" with no way to tell which compiled function it came from.
  console.error(`--- runtime error ---\n${err && err.stack || err}`);
  exitCode = 1;
  crashed = true;
}

process.stdout.write(stdout.join("\n") + (stdout.length ? "\n" : ""));
if (stderr.length) process.stderr.write(stderr.join("\n") + "\n");
if (crashed) process.exitCode = exitCode;
else process.exit(exitCode);
