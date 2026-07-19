# cbtc_train_presence — CBTC fleet-tracking case study

Communications-Based Train Control at the fleet-tracking layer. A bounded occupancy table records which train (if any) occupies each track section; the dispatcher issues placement and removal commands; the interlocking layer accepts only commands that keep the table in a safe state.

**Standard.** EN 50128 SIL 4 (same as ETCS movement authority in `examples/etcs_movement_authority/`).
**Ticket.** `mvl-lang/mvl#1910`.
**Sibling case at a different abstraction level.** `etcs_movement_authority` supervises one train against its Movement Authority; this example tracks the fleet.

## Files

- `model.mvl` — types (`OccupancyTable`, `PlacementCommand`, `RemoveCommand`, `UpdateResult`, `RejectReason`).
- `presence.mvl` — the kernel: IFC boundary, refinement-provable counter arithmetic, compound safety predicates, state-transition functions, inline unit tests.
- `main.mvl` — six scenarios walking through placement / removal / capacity / unauthorised.
- `presence_test.mvl` — end-to-end scenario tests.
- `Makefile` — standard targets plus `make test-50128` (SIL-4 assurance envelope).

## What is proven

`make prove` reports (production file only):

```
Summary: 6 proven (L1:4 L2:0 L3:0 L4:0 L5:2), 2 runtime, 0 failed
```

- **L1 (4 obligations)** — trivial literal subsumption on the counter helpers' ensures once their inputs are bounded literals in tests.
- **L5 (2 obligations)** — `incremented_count`'s and `decremented_count`'s post-conditions escalate to Z3. These are structurally in QF-LIA (Cooper QE territory); they escalate to L5 because MVL's L4 dispatcher does not currently split the conjunctive ensures across both bounds. Discharge succeeds either way.
- **runtime (2 obligations)** — call-site preconditions in `apply_placement` and `apply_removal`, one for each counter helper. MVL's inter-procedural refinement propagation does not currently narrow the interval on `table.total_trains` after the guard `if table.total_trains < 25`. Tracked in the MVL issue backlog (see `mvl-lang/mvl#1895`, `#1896`).

The runtime obligations become assertions in the compiled binary; the tests exercise them incidentally on every run.

## What is NOT proven (honest limits)

Two safety properties inherent to CBTC fleet tracking are not expressible as pointwise refinements at MVL's current capability:

1. **"No two sections hold the same train."** This is a `forall i, j` invariant over the occupancy array. MVL does not yet surface QF-Arrays refinements or bounded quantifiers over sequences. The current implementation preserves the invariant inductively through the update API (`apply_placement` only accepts a placement when `train_already_placed == false`, which the caller must derive from the actual array), but MVL cannot yet check the derivation at the type level. Full refinement-visible statement of this invariant awaits QF-Arrays surface work — the direct research motivation for ticket #1910.

2. **"Every train appears in at most one section."** Same shape as (1) — a cross-section invariant that quantifiers or array theory would express directly.

The physical predicates (`section_occupied`, `train_already_placed`, `section_currently_holds_target_train`) are accepted as boolean parameters into the kernel functions. A production version would compute them by querying the actual occupancy array, and MVL with QF-Arrays would let those queries participate in the refinement discharge.

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
Total: 6 proven (L1:4 L2:0 L3:0 L4:0 L5:2), 2 runtime, 0 failed

── (2) Behavioural unit tests (dynamic, specific inputs) ──────
test result: ok. 10 passed; 0 failed
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
