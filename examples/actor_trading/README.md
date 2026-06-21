# actor_trading

Verified order-matching engine — demonstrates **Req 10 refinement types** combined with **Phase 8 actors**.

**Phase 8 example** — requires actor runtime ([#695](https://github.com/LAB271/mvl_language/issues/695)).
Syntax is complete; codegen lands in #695 / #696.

---

## What this demonstrates

| Concept | Syntax | Purpose |
|---------|--------|---------|
| Refinement types | `type Price = Int where self > 0` | Compiler-enforced domain invariants — invalid prices are unrepresentable |
| `iso` ownership transfer | `pub fn submit(iso order: Order)` | Order moves between actors — no copying, no aliasing |
| `tag` references | `matcher: Matcher` in OrderBook | Actors hold identity handles, cannot read each other's internal state |
| Actor supervision | `OrderGateway → OrderBook → Matcher → RiskChecker` | Layered responsibility — each actor has one job |
| `concurrently { }` | Structured scope | All mailboxes drain before `main()` returns |
| Z3 verification | `verify.py` | Cross-field invariants proved for ALL inputs, not just test scenarios |

---

## Architecture

```
OrderGateway  ──iso order──►  OrderBook  ──tag matcher──►  Matcher
    │                                                          │
    │  new_bid(price, qty)                               iso fill
    │  new_ask(price, qty)                                     │
    │                                                          ▼
    │                                                   RiskChecker  ──► Console
    │
    └─ Req 10 enforced here: price > 0, quantity > 0
       Compiler rejects call sites that cannot prove these predicates.
```

### Actor responsibilities

| Actor | State | Behavior |
|-------|-------|----------|
| `OrderGateway` | `next_id` counter | Assigns IDs, validates refinements, forwards to OrderBook |
| `OrderBook` | `best_bid`, `best_ask` | Detects crosses, pairs orders, forwards pairs to Matcher |
| `Matcher` | `risk: RiskChecker` | Computes execution price/quantity, forwards Fill to RiskChecker |
| `RiskChecker` | `accepted`, `rejected` | Validates price invariant, prints execution report |

---

## Why Req 10 matters here

Without refinement types, the compiler cannot prevent:

```mvl
gw.new_bid(-50, 0)   // negative price, zero quantity — runtime crash or undefined behaviour
```

With inline `where` predicates on the gateway's parameters:

```mvl
pub fn new_bid(val price: Int where price > 0, val quantity: Int where quantity > 0) ! Console
```

A call like `gw.new_bid(-50, 0)` is a **compile error**:

```
error[E0301]: refinement predicate violated
   --> main.mvl
    | gw.new_bid(-50, 0)
    |            ^^^ cannot prove `-50 > 0`
```

The MVL layered solver (spec 018) handles these proofs at Layers 1–4 (no Z3 call needed for literal arguments).

The type aliases `Price`, `Quantity`, `OrderId` in `types.mvl` document the domain intent. They are not used on struct fields because MVL's method dispatch does not yet resolve through type aliases (e.g. `Price.to_string()` returns Unknown). Inline predicates on function parameters are the correct enforcement point.

---

## Two verification layers

| Layer | Tool | What it proves | Scope |
|-------|------|----------------|-------|
| Static (Req 10) | MVL compiler | `price > 0`, `quantity > 0`, `id >= 0` | Every constructor call site |
| SMT (Z3) | `verify.py` | Cross-field invariants (P1–P4) | All possible inputs |

### Verified properties (verify.py)

```
P1  No-worse-than-limit    fill.price ∈ [ask.price, bid.price]
P2  Quantity conservation  fill.quantity ≤ min(bid.quantity, ask.quantity)
P3  Price-time priority    crossing bid fills; non-crossing bid rests
P4  Monotonicity           higher bid never worsens fill price for ask side
```

P1 and P2 are proved by negation (UNSAT): Z3 finds no counterexample over all
integer inputs satisfying the scalar predicates. P3 and P4 are structural
consistency checks (SAT / UNSAT respectively).

---

## Running

```bash
# From the repo root:
make build

# Run the three trading scenarios:
cd examples/actor_trading
make run

# Prove invariants for ALL inputs (requires z3-solver):
pip install z3-solver
make verify
```

Expected output for `make run`:

```
=== Verified Trading Engine ===

--- scenario 1: resting ask crossed by aggressive bid ---
OrderBook: ask 0 price=100 qty=10
OrderBook: bid 1 price=102 qty=10
Matcher: executing bid 1 vs ask 0 @ 100 x 10
  FILL accepted  bid=1 ask=0 price=100 qty=10

--- scenario 2: resting bid crossed by aggressive ask ---
OrderBook: bid 2 price=99 qty=5
OrderBook: ask 3 price=97 qty=5
Matcher: executing bid 2 vs ask 3 @ 97 x 5
  FILL accepted  bid=2 ask=3 price=97 qty=5

--- scenario 3: no cross (bid below ask) ---
OrderBook: bid 4 price=95 qty=3
OrderBook: ask 5 price=98 qty=3
RiskChecker: 2 accepted, 0 rejected

Done.
```

Expected output for `make verify`:

```
actor_trading — Z3 invariant verification
==========================================

Proving properties over ALL possible order inputs:

  P1  PROVED  no-worse-than-limit: fill.price in [ask.price, bid.price]
  P2  PROVED  quantity conservation: fill.qty <= min(bid.qty, ask.qty)
  P3  PROVED  price-time priority: crossing bid fills, non-crossing bid rests
  P4  PROVED  monotonicity: higher bid never worsens fill price for ask side

All 4 properties proved.  Compilation artifact: invariants hold for ALL inputs.
```

---

## Capability summary

| Capability | Used for | Why |
|------------|----------|-----|
| `iso` | `Order`, `Fill` in actor messages | Each order/fill has exactly one owner at a time — no data races |
| `tag` | Actor references (`risk`, `matcher`, `book`) | Actors can dispatch to each other without sharing internal state |
| `val` | `bid_price`, `ask_price` in `RiskChecker.check` | Immutable scalars — safe to share, no ownership transfer needed |

---

## Related

- Issue: [#582 actor trading example](https://github.com/LAB271/mvl_language/issues/582)
- Epic: [#579 Phase 8 actor examples](https://github.com/LAB271/mvl_language/issues/579)
- Refinement solver spec: `.openspec/specs/018-refinement-solver/spec.md`
- Capabilities spec: `.openspec/specs/014-data-race-freedom/spec.md`
- Actor spec: `.openspec/specs/015-actors/spec.md`
- Basic actor example: [actor_pingpong](../actor_pingpong/)
- Actor runtime: [#695](https://github.com/LAB271/mvl_language/issues/695), [#696](https://github.com/LAB271/mvl_language/issues/696)
