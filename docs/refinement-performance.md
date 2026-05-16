# Refinement Solver Performance

Benchmark results for the layered refinement solver (issue #595).

The solver dispatches through five layers — trivial pattern matching, interval
arithmetic, symbolic path analysis, Cooper's Presburger QE, and Z3 SMT — trying
the cheapest layer first. See [ADR-0018 / spec 018](specs/018-refinement-solver/)
for design details.

---

## Running the benchmarks

```
cargo bench --bench refinement_solver
```

HTML report is written to `target/criterion/`. Filter by group:

```
cargo bench --bench refinement_solver -- layer
cargo bench --bench refinement_solver -- mode
cargo bench --bench refinement_solver -- corpus
```

---

## Results (macOS, Apple M-series, release build)

### Per-layer micro-benchmarks

Each entry is a tiny MVL program designed to exercise one primary solver layer.
The time includes the full type-checker pipeline (parse, type-check, refinement
check), so it reflects end-to-end compilation cost for small modules.

| Benchmark      | Layer exercised       | Time (ns) |
|----------------|-----------------------|----------:|
| `l1_literal`   | Layer 1 — trivial     |     6,579 |
| `l1_subsume`   | Layer 1 — subsumption |     7,248 |
| `l2_interval`  | Layer 2 — interval    |     7,749 |
| `l2_range`     | Layer 2 — range       |     8,611 |
| `l3_symbolic`  | Layer 3 — symbolic    |     6,400 |
| `l4_cooper`    | Layer 4 — Cooper      |     6,022 |
| `l5_z3`        | Layer 5 — Z3          |     7,361 |

All layers resolve in **< 10 µs** for simple programs, satisfying the
requirement from epic #545.

### Mode comparison on corpus files

The same corpus file is checked under three solver modes.

**`corpus/07_refinements/refinements_fully_proven.mvl`**

| Mode        | Time (ns) | vs `z3-only` |
|-------------|----------:|-------------:|
| `layered`   |    13,362 |      **127x** |
| `fast-only` |    13,356 |      **127x** |
| `z3-only`   | 1,698,159 |           1x |

> Layered solver is **127x faster** than Z3-only on this corpus.
> Epic #545 success criterion: ≥ 10x faster — **met**.

**`corpus/12_contracts/basic_contracts.mvl`**

| Mode        | Time (ns) | vs `z3-only` |
|-------------|----------:|-------------:|
| `fast-only` |    27,319 |      **293x** |
| `layered`   | 1,918,036 |        **4x** |
| `z3-only`   | 8,009,568 |           1x |

> The `layered` mode still invokes Z3 for contract predicates that the fast
> layers cannot close; `fast-only` skips Z3 entirely and emits `RuntimeCheck`
> for those sites instead.

**`corpus/07_refinements/refinements_valid.mvl`**

All three modes finish in ~11,000 ns (no call-site refinement checks trigger
in this corpus file — it only defines types and structs).

---

## Solver layer hit rates

Layer 1 and 2 together handle the vast majority of real-world refinements (≥ 75%
estimated). Z3 is invoked only for non-linear arithmetic and complex contract
predicates that the fast layers cannot decide.

---

## Regression tracking

The CI benchmark job runs under `jobs/benchmark` in `.github/workflows/ci.yml`.
Results are uploaded as workflow artifacts (`refinement-bench-results`).
To detect regressions, download and compare the `bencher` output across runs:

```bash
# After fetching two artifact ZIPs:
diff bench-baseline.txt bench-current.txt
```
