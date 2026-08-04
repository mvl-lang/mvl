// runtime/wasm-browser/smoke_test.mjs — end-to-end verification for
// `make test-runtime-wasm-browser` (#2093 Phase 2, ADR-0063).
//
// Node's WebAssembly implementation is the same one browsers use, and Node
// has no built-in WASI shim active here (we supply our own), so this is a
// faithful stand-in for "does this actually work in a browser" without
// needing a browser.
//
// Usage: MVL_RUNTIME_WASM_BROWSER=<path to mvl_runtime_wasm.wasm> node smoke_test.mjs
// (set by `make test-runtime-wasm-browser`)

import { readFile } from "node:fs/promises";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { instantiateMvlProgram } from "./runtime.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const runtimePath = process.env.MVL_RUNTIME_WASM_BROWSER;
if (!runtimePath) {
  console.error("MVL_RUNTIME_WASM_BROWSER is not set");
  process.exit(1);
}

const mvlBin = path.join(here, "..", "..", "target", "debug", "mvl");
const smokeMvl = path.join(here, "smoke.mvl");
const smokeWat = path.join(here, "smoke.wat");

execFileSync(mvlBin, ["build", smokeMvl, "--backend=wasm", "-o", smokeWat], {
  cwd: here,
  env: { ...process.env, MVL_NO_REEXEC: "1" },
  stdio: "inherit",
});

const smokeWasm = path.join(here, "smoke.wasm");
execFileSync("wasm-tools", ["parse", smokeWat, "-o", smokeWasm]);

const runtimeBytes = await readFile(runtimePath);
const programBytes = await readFile(smokeWasm);

const output = [];
const originalLog = console.log;
console.log = (...args) => output.push(args.join(" "));

const { programInstance } = await instantiateMvlProgram(runtimeBytes, programBytes);
programInstance.exports._start();

console.log = originalLog;
output.forEach((line) => console.log(line));

const expected = [
  "hello from wasm-browser",
  "clock ok",
  "random.int ok",
  "random.choice ok",
];
const missing = expected.filter((line) => !output.includes(line));
const failed = output.filter((line) => line.includes("FAILED"));

if (missing.length > 0 || failed.length > 0) {
  console.error("SMOKE TEST FAILED");
  console.error("missing:", missing);
  console.error("failed lines:", failed);
  process.exit(1);
}

console.log("wasm-browser smoke test: OK");
