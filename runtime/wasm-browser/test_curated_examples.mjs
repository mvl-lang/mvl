// runtime/wasm-browser/test_curated_examples.mjs — smoke-tests the exact
// example apps mvl-lang/mvl-playground curates in its Examples dropdown
// (mvl-playground#36's web/scripts/sync-examples.sh) against the real
// wasm-browser runtime, end to end under Node (#2093 Phase 2, ADR-0063).
//
// This is a "does it actually run" guarantee, not output-correctness
// per example: build via `mvl build --backend=wasm --target=wasm-browser`,
// instantiate with the same runtime.mjs shim a browser would use, call the
// WASI entry point, and confirm it completes without an uncaught trap and
// (mirroring the corresponding sibling in medical_triage, which asserts
// nothing about its own values) without a "panic"/"unreachable" surfacing
// on stderr.
//
// Usage: MVL_RUNTIME_WASM_BROWSER=<path> node test_curated_examples.mjs
// (set by `make test-wasm-browser`)

import { readFile, mkdtemp, rm } from "node:fs/promises";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";
import path from "node:path";
import { instantiateMvlProgram } from "./runtime.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.join(here, "..", "..");
const runtimePath = process.env.MVL_RUNTIME_WASM_BROWSER;
if (!runtimePath) {
  console.error("MVL_RUNTIME_WASM_BROWSER is not set");
  process.exit(1);
}

// Mirrors web/scripts/sync-examples.sh's curated list in
// mvl-lang/mvl-playground (issue #8 / PR #36) — same 6 apps, same names
// modulo etcs_move_authority (mvl-playground's manifest currently spells
// it etcs_movement_authority; the directory here is the source of truth).
//
// `knownIssue` marks a documented, tracked xfail — NOT a silent exclusion
// (the example still builds and runs every time this target does; its
// output/trap still prints). Remove the field once the linked issue is
// fixed; if the example starts passing before that, this script flags it
// as an unexpected pass so the marker doesn't go stale.
const CURATED_EXAMPLES = [
  { name: "hello_world", entry: "examples/hello_world.mvl" },
  { name: "actor_pingpong", entry: "examples/actor_pingpong/main.mvl" },
  { name: "flight_fuel_planning", entry: "examples/flight_fuel_planning/main.mvl" },
  { name: "etcs_move_authority", entry: "examples/etcs_move_authority/main.mvl" },
  { name: "pci_payment", entry: "examples/pci_payment/main.mvl" },
  { name: "medical_triage", entry: "examples/medical_triage/main.mvl" },
];

const mvlBin = path.join(repoRoot, "target", "debug", "mvl");
const runtimeBytes = await readFile(runtimePath);

async function runOne({ name, entry }) {
  const entryAbs = path.join(repoRoot, entry);
  const workDir = await mkdtemp(path.join(tmpdir(), `mvl-wasm-browser-${name}-`));
  const stem = path.basename(entry, ".mvl");
  const watPath = path.join(workDir, `${stem}.wat`);
  const wasmPath = path.join(workDir, `${stem}.wasm`);

  try {
    const stdout = [];
    const stderr = [];
    const originalLog = console.log;
    const originalError = console.error;
    console.log = (...args) => stdout.push(args.join(" "));
    console.error = (...args) => stderr.push(args.join(" "));

    let failure = null;
    try {
      execFileSync(
        mvlBin,
        ["build", entryAbs, "--backend=wasm", "--target=wasm-browser", "-o", watPath],
        { cwd: workDir, env: { ...process.env, MVL_NO_REEXEC: "1" }, stdio: ["ignore", "pipe", "pipe"] },
      );
      execFileSync("wasm-tools", ["parse", watPath, "-o", wasmPath]);

      const programBytes = await readFile(wasmPath);
      const { programInstance } = await instantiateMvlProgram(runtimeBytes, programBytes);
      programInstance.exports._start();
    } catch (err) {
      failure = err;
    } finally {
      console.log = originalLog;
      console.error = originalError;
    }

    const panicked = stderr.some((l) => /panic|unreachable/i.test(l));
    const ok = !failure && !panicked;
    return { name, ok, failure, stdout, stderr };
  } finally {
    await rm(workDir, { recursive: true, force: true });
  }
}

// Same `%-20s  <color>SYMBOL  LABEL</color>` layout `examples/test-all.sh`
// uses for `make test-examples-wasm` (and every other `test-examples-*`
// suite) — one visual vocabulary for "did this example's backend run pass"
// across both entry points instead of this script inventing its own.
const GREEN = "\x1b[32m";
const RED = "\x1b[31m";
const YELLOW = "\x1b[33m";
const RESET = "\x1b[0m";

let unexpectedFailures = 0;
let stalePasses = 0;
for (const example of CURATED_EXAMPLES) {
  const result = await runOne(example);
  const namePad = example.name.padEnd(20);
  if (result.ok && !example.knownIssue) {
    console.log(`  ${namePad}  ${GREEN}✓  PASS${RESET}`);
  } else if (result.ok && example.knownIssue) {
    stalePasses++;
    console.log(
      `  ${namePad}  ${YELLOW}✓  XPASS${RESET}  issue #${example.knownIssue} looks fixed — remove its knownIssue marker`,
    );
  } else if (!result.ok && example.knownIssue) {
    console.log(`  ${namePad}  ${YELLOW}~  XFAIL${RESET}  known issue #${example.knownIssue}, tracked — not blocking`);
  } else {
    unexpectedFailures++;
    console.log(`  ${namePad}  ${RED}✗  FAIL${RESET}`);
    if (result.failure) {
      const msg = result.failure.stderr?.toString?.() || result.failure.message || String(result.failure);
      msg.trim().split("\n").slice(-5).forEach((l) => console.log(`         ${l}`));
    }
    result.stderr.forEach((l) => console.log(`         stderr: ${l}`));
  }
}

console.log("");
if (unexpectedFailures > 0) {
  console.log(`  ${RED}✗  ${unexpectedFailures} of ${CURATED_EXAMPLES.length} curated example(s) failed unexpectedly under wasm-browser${RESET}\n`);
  process.exit(1);
}
if (stalePasses > 0) {
  console.log(`  ${RED}✗  ${stalePasses} curated example(s) marked knownIssue now pass — update test_curated_examples.mjs${RESET}\n`);
  process.exit(1);
}
console.log(`  ${GREEN}✓  All ${CURATED_EXAMPLES.length} curated example(s) passed under wasm-browser${RESET}\n`);
