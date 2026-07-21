# cbtc_train_presence — CBTC fleet-tracking case study

Communications-Based Train Control at the fleet-tracking layer. A bounded occupancy table records which train (if any) occupies each track section; the dispatcher issues placement and removal commands; the interlocking layer accepts only commands that keep the table in a safe state.

**Standard.** EN 50128 SIL 4 (same as ETCS movement authority in `examples/etcs_movement_authority/`).
**Ticket.** `mvl-lang/mvl#1910`.
**Sibling case at a different abstraction level.** `etcs_movement_authority` supervises one train against its Movement Authority; this example tracks the fleet.

## Files

- `model.mvl` — types (`OccupancyTable`, `PlacementCommand`, `RemoveCommand`, `UpdateResult`, `RejectReason`).
- `presence.mvl` — the kernel: IFC boundary, refinement-provable counter arithmetic, compound safety predicates, state-transition functions, inline unit tests.
- `invariants.mvl` — QF-Arrays surface: `require_dense_fleet`, `section_slot`, `empty_fleet`, `make_dense_fleet`, six inline unit tests exercising bounded-quantifier refinements (#1915) and array-index refinements (#1916).
- `main.mvl` — six scenarios walking through placement / removal / capacity / unauthorised.
- `presence_test.mvl` — end-to-end scenario tests.
- `Makefile` — standard targets plus `make test-50128` (SIL-4 assurance envelope).

## What is proven

`make prove` reports (per production file):

```
presence.mvl:   6 proven (L1:4 L2:0 L3:0 L4:0 L5:2), 2 runtime, 0 failed
invariants.mvl: 1 proven (L1:1 L2:0 L3:0 L4:0 L5:0), 3 runtime, 0 failed
Total:          7 proven (L1:5 L2:0 L3:0 L4:0 L5:2), 5 runtime, 0 failed
```

**`presence.mvl`**

- **L1 (4 obligations)** — trivial literal subsumption on the counter helpers' ensures once their inputs are bounded literals in tests.
- **L5 (2 obligations)** — `incremented_count`'s and `decremented_count`'s post-conditions escalate to Z3. These are structurally in QF-LIA (Cooper QE territory); they escalate to L5 because MVL's L4 dispatcher does not currently split the conjunctive ensures across both bounds. Discharge succeeds either way.
- **runtime (2 obligations)** — call-site preconditions in `apply_placement` and `apply_removal`, one for each counter helper. MVL's inter-procedural refinement propagation does not currently narrow the interval on `table.total_trains` after the guard `if table.total_trains < 25`. Tracked in the MVL issue backlog (see `mvl-lang/mvl#1895`, `#1896`).

**`invariants.mvl`**

- **L1 (1 obligation)** — `section_slot`'s index-bounds precondition (`idx >= 1 && idx <= 50`) is discharged trivially at L1 when the call site passes a literal index in range.
- **runtime (3 obligations)** — the three `require_dense_fleet` call sites. Layer 3 (#1915) unrolls `forall i in [1..50]. sections.get(i) != None` into 50 instantiated conjuncts; each `sections.get(i) != None` atom is opaque to the solver (static bounds reasoning only, per #1916), so all conjuncts fall to RuntimeCheck. `make prove` groups these as one runtime obligation per call site. See the §QF-Arrays coverage section below for the full analysis.

The runtime obligations become assertions in the compiled binary; the tests exercise them on every run.

## QF-Arrays coverage

`invariants.mvl` earns the QF-Arrays row in the refinements paper's coverage matrix by exercising two compiler features together for the first time:

```mvl
pub total fn require_dense_fleet(sections: List[OccupancyEntry]) -> Bool
    requires forall i in [1..50]. sections.get(i) != None
{ true }
```

**What the solver sees.** Layer 3 (#1915) expands the `forall` into 50 conjuncts:
```
sections.get(1) != None  ∧  sections.get(2) != None  ∧  …  ∧  sections.get(50) != None
```
Each conjunct is an array-index atom of the form `list.get(k) != None`.  Per the #1916 merge note ("bounds reasoning is static-only"), such atoms are opaque to the solver when the list contents are not statically known.  The 50 conjuncts therefore fall to RuntimeCheck collectively — `make prove` reports them as a single grouped runtime obligation per call site.

**Fourth runtime-obligation phenomenon.** The three call sites in the test suite generate three runtime obligations, making this the fourth named runtime-obligation phenomenon in the refinements paper's taxonomy, and the first *compiler-side* one:

| # | Name | Origin |
|---|------|---------|
| 1 | Interval-guard narrowing | QF-NIA solver side (`apply_placement`, `apply_removal`) |
| 2 | Conjunctive ensures splitting | QF-LIA solver side (not present here) |
| 3 | Z3 non-linear arithmetic | QF-NIA solver side (not present here) |
| **4** | **Bounded-quantifier expansion over opaque array-index atoms** | **Compiler side (`require_dense_fleet`)** |

The first three phenomena are QF-NIA or QF-LIA solver gaps.  This fourth phenomenon is a compiler gap: L3's expansion produces well-formed conjuncts, but each conjunct is opaque because `list.get(k)` carries no static content bound.

**Current limitation.** MVL's `forall` body admits comparisons, boolean connectives, and array-index atoms, but does not yet admit user-defined function calls.  A `valid_slot(i)` helper cannot appear inside the body today — the predicate must be inlined.

## What is NOT proven (honest limits)

One fleet-wide safety property is now expressible as a bounded-quantifier refinement (#1915 + #1916), though it falls to runtime.  One is still beyond current capability:

1. **"Every section slot has an entry"** (`forall i in [1..50]. sections.get(i) != None`) — now expressible in `require_dense_fleet`.  Falls to runtime because the array-index atoms are opaque to the solver.  Tracked in the paper as the fourth runtime-obligation phenomenon (see §QF-Arrays coverage above).

2. **"No two sections hold the same train."** This is a `forall i, j` invariant — a cross-section predicate over two array indices simultaneously.  MVL's bounded quantifier form (`forall i in [lo..hi]. expr`) handles a single index variable; two-variable quantifiers (`forall i, j`) remain unsupported.  The implementation preserves this invariant inductively through the update API (`apply_placement` only accepts a placement when `train_already_placed == false`), but MVL cannot yet check the derivation at the type level.

3. **"Every train appears in at most one section."** Same shape as (2) — a cross-section invariant that requires two-variable quantifiers or array theory beyond what #1915 provides.

The physical predicates (`section_occupied`, `train_already_placed`, `section_currently_holds_target_train`) are accepted as boolean parameters into the kernel functions. A production version would compute them by querying the actual occupancy array, and MVL with full QF-Arrays would let those queries participate in the refinement discharge.

## IFC boundary

Dispatcher commands arrive from the control-centre RTU as `Tainted[PlacementCommand]` and `Tainted[RemoveCommand]`. Two audit anchors:

- `DISPATCHER-CMD-001` — placement declassification via `admit_placement_command`.
- `DISPATCHER-CMD-002` — remove declassification via `admit_remove_command`.

Reproduce the audit:

```bash
grep -n "DISPATCHER-CMD-" presence.mvl
```

Returns exactly two lines. Every trust-boundary crossing on dispatcher commands is grep-able.

## MC/DC

Compound decision under audit: `can_place(section_occupied, train_already_placed, at_capacity, authorised)` — four atomic conditions, each in a single clause, no structural coupling. Unique-cause MC/DC is achievable in principle.

**Current status:** `make mcdc` returns "No compound boolean conditions found — no MC/DC obligations." This is a known gap in the MVL MC/DC tool (`mvl-lang/mvl#1888`) — the tool scans test files for compound decisions, and imports from the production sibling module do not propagate the decision graph. The same gap affects `dose_scheduling` and other current examples. The inline `test fn` blocks in `presence.mvl` are structured to activate MC/DC discovery once the tool's sibling-loading pass lands. No workaround required in this example.

`MCDC-CBTC-001` documents the single compound decision under audit; grepping for it returns the anchor line.

## Standard mapping (EN 50128 SIL 4)

`make test-50128` composes the SIL-4 assurance envelope:

```
── (1) Static refinement proof (compile-time, all inputs) ─────
Total: 7 proven (L1:5 L2:0 L3:0 L4:0 L5:2), 5 runtime, 0 failed

── (2) Behavioural unit tests (dynamic, specific inputs) ──────
test result: ok. 24 passed; 0 failed
All tests passed.

── (3) Branch coverage (decision points reached) ──────────────
(see `make coverage`)

── (4) MC/DC coverage (compound decisions, --masking) ─────────
(blocked by #1888 — see above)
```

Two of the four axes are load-bearing today; MC/DC recovers on #1888 landing.

## Running the demo

```bash
make run
```

Produces:
```
1. normal placement:         applied (total_trains=4)
2. section already occupied: rejected: section already occupied
3. train already placed:     rejected: train already placed elsewhere
4. at capacity:              rejected: fleet capacity reached
5. unauthorised:             rejected: not authorised
6. normal removal:           applied (total_trains=2)
```

## Design decisions worth noting

**Boolean parameters for derived facts.** The kernel functions accept `section_occupied`, `train_already_placed`, and `section_currently_holds_target_train` as booleans rather than querying the occupancy array. This is a deliberate abstraction — it factors the array-query concern out of the safety decision. When MVL grows QF-Arrays refinements, the boolean parameters can be replaced with `select` calls whose validity the checker proves against the array-theory obligations.

**Counter tracking as a separate refinement obligation.** `total_trains` is a separate integer with its own bounded refinement, incremented and decremented by helper functions with proved ensures. This is the pattern that mirrors dose_scheduling's finding: refinement placement determines which layer discharges. Explicit counter arithmetic gives clean L4/L5 obligations; a raw array-length would give QF-Arrays obligations MVL doesn't yet surface.

**Priority ordering in rejection reasons.** `placement_reject_reason` prioritises authorisation before physical checks. This avoids leaking physical-state information to unauthorised callers via error codes — a small IFC hygiene point that matters in real dispatch systems.
