# Experiment 008 — Standalone MVL-WASM Example Harness

A second, independent host for `mvl build --backend=wasm` output, outside a browser.
Motivated directly by experiments 004-007: this repo spent a whole session
understanding WASM execution mechanics (static-page delivery, stdout capture,
`Worker.terminate()` timing, runtime strategy) using deliberately tiny test
programs. This experiment applies that understanding to the actual question those
experiments were building toward — **does the WASM backend hold up on real,
non-trivial MVL programs**, not just hello-world-scale ones — and is built so the
same approach could be folded into `mvl-lang/mvl-playground` later as an automated
check, rather than the playground's 12 curated examples being the only proof the
WASM backend works on anything.

## The gap this closes

`mvl-lang/mvl-playground` is currently the **only** thing anywhere that can execute
`mvl build --backend=wasm` output. Confirmed directly, not assumed:

- `mvl run` doesn't accept `--backend=wasm` at all (not in its own `--help`)
- Bare `wasmtime run main.wasm` fails: `unknown import: runtime::memory has not been defined` — the compiled module expects a `runtime` import namespace (~60 hand-written functions for string/array/option/result/map operations) that is a **playground-specific convention**, not a standard WASI world. Nothing outside the playground's own JS implements it.

So "does this MVL program actually run correctly under the WASM backend" has never
been testable for anything outside the playground's 12 curated examples — which are,
by construction, the *only* ones anyone has ever run this way.

## What this harness is

- **`harness/mvl-runtime.js`** — a byte-faithful port of `mvl-lang/mvl-playground`'s
  `web/src/runtime/mvl-runtime.ts` (TS → plain JS, no logic changes — every function
  ported 1:1). This tests the actual runtime the playground ships, not a
  reimplementation of it. If `mvl-runtime.ts` changes upstream, this file needs the
  same change, or it stops being a faithful test of what the playground actually
  does.
- **`harness/run-example.mjs`** — compiles an `.mvl` file (`mvl build --backend=wasm`),
  converts WAT → WASM binary (`wasm-tools parse`, the CLI-equivalent of what the
  playground's Rust backend does server-side via the `wat` crate), instantiates
  against the ported runtime + `@bjorn3/browser_wasi_shim` (same shim experiments
  004-007 used, works identically in plain Node — no browser needed for any of
  this), and captures stdout/stderr.
- **`run-tests.sh`** — for each `test-cases/<name>/`, runs it and diffs captured
  output against `expected-stdout.txt` (captured from `mvl run main.mvl` — the
  native/Rust-backend execution, ground truth for correct behavior).

No browser, no Playwright, no headless Chromium anywhere in this experiment — a
genuinely lighter-weight testing approach than the playground's own Worker-based
execution, since none of what's being tested here (does the compiled module run
correctly against the runtime convention) actually needs a browser context.

## Test case: actor_trading

Chosen deliberately as something with real complexity, not another hello-world:
`mvl-lang/mvl/examples/actor_trading` is an order-matching engine with real actors
(`OrderBook`, `Matcher`), not the toy ping-pong of `actor_pingpong` (the only
actor-based example currently curated into the playground).

**Original result (superseded below): FAIL, filed as [mvl-lang/mvl#2083](https://github.com/mvl-lang/mvl/issues/2083)** —
a trap in `order_book_submit` during the first actor-to-actor message dispatch, with
what looked like solid evidence it wasn't a missing import, a memory-capacity issue,
or one of three already-tracked WASM gaps. **That issue has since been corrected and
closed — see "Fix: `_mvl_struct_alloc`/`_mvl_array_get`" below. It was a bug in this
harness, not the compiler.** Left the original repro details out of this README
(they're preserved in the issue's history) since they'd otherwise read as evidence
for a conclusion this file no longer holds.

**Current result, after both runtime fixes below: no crash.** All three scenarios
run to completion, and every printed value (prices, quantities, bid/ask ids, fill
messages) matches a native `mvl run main.mvl` execution exactly. The one remaining
difference is explained in "A genuine backend difference: actor output ordering" —
it isn't a bug, and isn't what #2083 was about.

## Fix: Option/Result tag polarity was inverted (found while building experiment 010)

While building experiment 010 (mastermind_web), reverse-engineering the WASM ABI
for a struct-returning function required reading the compiler's actual WAT output
for a trivial probe (`xs.get(i).unwrap_or(dflt)`). That surfaced the compiled code:

```wat
call $_mvl_option_tag
i32.eqz
if (result i64)          ;; taken when tag == 0
  call $_mvl_option_value_i64
else                      ;; taken when tag != 0
  local.get $dflt
end
```

`i32.eqz` branches on `tag == 0`, and that branch is the one that reads the value —
so the compiler's real convention is **0 = Some/Ok (has a value), 1 = None/Err**.

This harness's `mvl-runtime.js` (and, checked directly, `mvl-lang/mvl-playground`'s
actual production `web/src/runtime/mvl-runtime.ts`, lines 236-238 and 304 at the time
of writing) had it backwards: `_mvl_option_some_i64/i32` stored tag `1`,
`_mvl_option_none` stored tag `0`, `_mvl_array_get_option_i64/i32` and
`_mvl_map_get_si64` used found=`1`/not-found=`0`, and the equivalent Result
functions (`_mvl_result_ok_*`/`_mvl_result_err_str`, used by
`_mvl_string_parse_int`) had the same inversion.

**Effect: silently wrong values, not a crash.** A present element compared as
absent (falling through to the `unwrap_or` default) and vice versa — for
`.get(i).unwrap_or(...)`, map lookups, and `String.parse_int()` alike. Verified
both directions empirically with isolated single-function probes
(`xs.get(i).unwrap_or(dflt)` and `s.parse_int().unwrap_or(dflt)`) before and after
the fix, not assumed from reading code.

**Fixed here**: flipped the tag argument in every `storeOption`/`storeResult` call
in `harness/mvl-runtime.js`, including the `?? 0` fallback in `_mvl_option_tag`/
`_mvl_result_tag` (now `?? 1`, so a missing/invalid handle fails safe to
None/Err instead of defaulting to Some/Ok).

**Regression test**: `test-cases/option_probe/` — a minimal `.get().unwrap_or()`
program that fails (both directions) against the pre-fix runtime and passes
against the fix (verified both ways via `git stash` before committing).

**Not fixed here**: `mvl-lang/mvl-playground`'s own `runtime.ts` has the identical
bug and is production code — out of scope for this repo to patch directly, flagged
separately.

## Fix: `_mvl_struct_alloc`/`_mvl_array_get`/string-creation functions used a handle table instead of real memory (this closed and corrected mvl#2083)

Found the same way as the Option/Result fix above: reverse-engineering `score_guess`'s
WASM ABI for experiment 010 required reading its actual WAT body. That surfaced:

```wat
i32.const 16
call $_mvl_struct_alloc
local.set $__st
local.get $__st
local.get $blacks
i64.store offset=0        ;; raw store directly on _mvl_struct_alloc's return value
local.get $__st
local.get $whites
i64.store offset=8
```

`_mvl_struct_alloc`'s return value is **not** an opaque handle the compiled module
hands back to JS to interpret — the module itself does raw `i64.store`/`i64.load` on
it. It has to be a real address in the shared linear memory. Same pattern confirmed
for two more import functions by reading their call sites: `_mvl_array_get` (used by
`for x in [array literal]` loops — `call $_mvl_array_get` / `i64.load offset=0`) and
every string-*creating* function (`_mvl_string_new/concat/substring/to_upper/
to_lower/trim/replace` — `s.concat(s)` compiles to `call $_mvl_string_concat` /
`i32.load offset=0` (ptr) / `i32.load offset=4` (len), an 8-byte `{ptr, len}`
descriptor record in memory, not a handle).

This harness's `mvl-runtime.js` used the same handle-table pattern (a shared
`nextHandle` counter) for **all** `_mvl_*_alloc`/`_mvl_*_new`-style functions,
correct for the ones only ever read back through another runtime *function call*
(`_mvl_option_value_i64`, `_mvl_array_get_option_i64`, etc. — genuinely fine as JS
Map keys) but wrong for these three, whose return values the compiled module
dereferences directly as memory addresses. A handle like `3` used as a raw address
corrupts real module memory — those low addresses overlap static rodata and other
live data in any nontrivial module.

**This is what #2083 actually was.** `actor_trading`'s `main.wat` calls
`$_mvl_struct_alloc` 18 times (`Order` and `Fill` are both structs) — this harness
had been running that test against a broken allocator for the entire time the crash
was diagnosed and filed as an "actor message routing" compiler bug. After this fix:
no crash, full run to completion, output values matching native exactly. **Issue
closed with a correcting comment** — see the commit/PR for the link. Own the
mistake: should have ruled out the harness's own allocator more thoroughly before
filing against the compiler.

**Fixed here**: `_mvl_struct_alloc` and `_mvl_array_get` now use a real bump
allocator (`bumpAllocScratch`) into the shared `WebAssembly.Memory`, starting at a
fixed offset (32KB — comfortably above any static rodata a realistically-sized
example emits, and inside the initial 64KB page so no `memory.grow` is needed for
typical small examples) and growing memory on demand for larger ones.
String-creating functions now encode to UTF-8, write the bytes into that same
scratch space, and return a `{ptr, len}` descriptor pointer instead of a Map handle.
Never frees (fine for a short-lived test-harness process, not a real GC — documented
as a limitation, not silently swept under the rug).

**Not fixed here**: `mvl-lang/mvl-playground`'s own `runtime.ts` has the identical
`_mvl_struct_alloc` handle-table bug (checked directly) — production code, out of
scope for this repo, flagged separately alongside the Option/Result bug.

## A genuine backend difference: actor console-output ordering is nondeterministic in native, deterministic in WASM

After the fix above, `actor_trading`'s WASM output still doesn't byte-diff clean
against the checked-in `expected-stdout.txt` — but the reason turned out to be a
property of the *native* backend, not a WASM defect. Running `mvl run main.mvl`
three times back to back produces **two different outputs** (confirmed via
`md5`sum of three consecutive runs: run 1 and run 3 matched, run 2 didn't) — actor
mailbox draining interleaves nondeterministically with the driving code's own
`println` calls natively. Running the WASM harness three times back to back
produces the **same output every time** — the compiled module's actor pump appears
to run each mailbox fully synchronously in-order, with no interleaving to be
nondeterministic about.

Practically: comparing WASM output against a single captured native run is not a
meaningful correctness bar for this actor-based example — the "expected" file itself
isn't stable. The values are what matter (and they match), not the interleaving
order. Left `expected-stdout.txt` as originally captured (one valid native ordering
among several) rather than chase a moving target; `run-tests.sh` falls back to a
sorted-line comparison when the exact-sequence diff fails, so this case reports
PASS (with the order-only diff still printed for visibility) instead of failing on
a difference that isn't a defect.

## Usage

```bash
cd harness && npm install    # once
cd ..
./run-tests.sh                # runs every test-cases/*/, reports PASS/FAIL with diffs
```

To try a different example: copy its `.mvl` files into `test-cases/<name>/`, capture
its correct output via `mvl run main.mvl` into `test-cases/<name>/expected-stdout.txt`
(strip the compiler's own build-status lines from the top), and re-run.

## Structure

```
008_mvl_example_wasm_harness/
├── README.md
├── run-tests.sh
├── harness/
│   ├── mvl-runtime.js       # ported from mvl-playground's mvl-runtime.ts
│   ├── run-example.mjs      # compile -> WAT->WASM -> instantiate -> capture
│   └── package.json         # @bjorn3/browser_wasi_shim
└── test-cases/
    └── actor_trading/
        ├── main.mvl          # copied from mvl-lang/mvl/examples/actor_trading
        ├── types.mvl
        └── expected-stdout.txt  # captured via `mvl run main.mvl`
    # main.wat / main.wasm are run-example.mjs output, gitignored
```

## What could move into mvl-playground later

The `runtime` namespace convention and the WAT→WASM→instantiate pipeline are
identical to what the playground already does — this harness's real value if
adopted there is as a **CI check independent of the browser**: run every example a
future curator considers adding through `run-tests.sh` before it's wired into
`sync-examples.sh`, so a broken example is caught by a fast Node script instead of a
silently blank Runtime tab in a real user's browser.
