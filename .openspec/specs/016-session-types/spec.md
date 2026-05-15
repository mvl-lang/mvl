---
domain: language
version: 0.1.0
status: draft
date: 2026-05-15
---

# 016 — Session Types (Phase 8)

Session types (Honda 1993) add typed communication protocols to MVL.  A session
type describes the exact sequence of messages that two participants must exchange
on a channel.  The compiler verifies that both sides follow the declared protocol;
deviating from the protocol (wrong type, wrong order, missing message) is a
compile error.

## Notation

| Syntax | Name | Meaning |
|--------|------|---------|
| `!T. S` | Send | Send a value of type `T`, then continue as `S` |
| `?T. S` | Receive | Receive a value of type `T`, then continue as `S` |
| `+{ l1: S1, l2: S2 }` | Internal choice | This side selects a branch |
| `&{ l1: S1, l2: S2 }` | External choice | The other side selects a branch |
| `end` | End | Protocol complete; channel closed |

The `.` combinator chains steps: `!Int. ?Bool. end` means "send an Int, then
receive a Bool, then the protocol terminates".

## Duality

Every protocol has two complementary sides.  Given one side `S`, the other is
`dual(S)`:

| Side A | Side B = dual(A) |
|--------|-----------------|
| `!T. S` | `?T. dual(S)` |
| `?T. S` | `!T. dual(S)` |
| `+{ l: S }` | `&{ l: dual(S) }` |
| `&{ l: S }` | `+{ l: dual(S) }` |
| `end` | `end` |

Example — a buyer/seller protocol:

```mvl
// Buyer's side: send Request, receive Quote, then choose
type BuyProtocol = !Request. ?Quote. +{
    accept: !Payment. ?Receipt. end,
    reject: end
}

// Seller's side: the exact dual of BuyProtocol
type SellProtocol = ?Request. !Quote. &{
    accept: ?Payment. !Receipt. end,
    reject: end
}
```

## Requirements

### Requirement 1: Session Type Declaration Syntax [MUST]

The compiler MUST parse session type aliases using the Honda 1993 notation.
A session type may appear anywhere a `TypeExpr` is valid (type aliases, function
parameters, struct fields).

Supported forms:
- `!T. S` — send prefix
- `?T. S` — receive prefix
- `+{ label: S, ... }` — internal choice (one or more branches)
- `&{ label: S, ... }` — external choice (one or more branches)
- `end` — terminal

**Implementation:** `src/mvl/parser/ast.rs::TypeExpr::Session`,
`src/mvl/parser/ast.rs::SessionOp`,
`src/mvl/parser/types.rs::parse_session_op`

#### Scenario: Simple send/receive protocol

- GIVEN `type Ping = !Int. ?Bool. end`
- WHEN the parser processes this declaration
- THEN it MUST produce `TypeBody::Alias(TypeExpr::Session { op: Send { msg: Int, cont: Receive { msg: Bool, cont: End } } })`

**Tests:** `src/mvl/parser/types.rs::tests::session_send_int_end`,
`src/mvl/parser/types.rs::tests::session_receive_bool_end`,
`src/mvl/parser/types.rs::tests::session_send_receive_sequence`

#### Scenario: Internal and external choice

- GIVEN `type P = +{ accept: !Int. end, reject: end }`
- WHEN the parser processes this declaration
- THEN it MUST produce an `InternalChoice` node with two branches: `accept` and `reject`

- GIVEN `type Q = &{ ok: ?String. end, err: end }`
- WHEN the parser processes this declaration
- THEN it MUST produce an `ExternalChoice` node

**Tests:** `src/mvl/parser/types.rs::tests::session_internal_choice`,
`src/mvl/parser/types.rs::tests::session_external_choice`

---

### Requirement 2: Session Type Resolution [MUST]

The checker MUST resolve `TypeExpr::Session` to `Ty::Session(SessionTy)`.
The `SessionTy` tree mirrors the `SessionOp` tree with payload types fully
resolved to checker `Ty` values.

**Implementation:** `src/mvl/checker/types.rs::Ty::Session`,
`src/mvl/checker/types.rs::SessionTy`,
`src/mvl/checker/types.rs::resolve_session_op`

#### Scenario: Resolve send/receive protocol

- GIVEN `type Ping = !Int. ?Bool. end`
- WHEN the checker resolves the type alias
- THEN it MUST produce `Ty::Session(SessionTy::Send(Ty::Int, SessionTy::Receive(Ty::Bool, SessionTy::End)))`

**Tests:** `src/mvl/checker/types.rs::tests::resolve_session_send_int_end`

---

### Requirement 3: Duality [MUST]

The checker MUST compute the dual of any `SessionTy`.

Rules:
- `dual(!T. S) = ?T. dual(S)`
- `dual(?T. S) = !T. dual(S)`
- `dual(+{ l: S, ... }) = &{ l: dual(S), ... }`
- `dual(&{ l: S, ... }) = +{ l: dual(S), ... }`
- `dual(end) = end`

**Implementation:** `src/mvl/checker/types.rs::SessionTy::dual`,
`src/mvl/checker/session.rs::check_dual`

#### Scenario: Duality of send is receive

- GIVEN `SessionTy::Send(Int, End)`
- WHEN `dual()` is called
- THEN the result MUST be `SessionTy::Receive(Int, End)`

#### Scenario: Internal choice duals to external

- GIVEN `SessionTy::InternalChoice([("a", End)])`
- WHEN `dual()` is called
- THEN the result MUST be `SessionTy::ExternalChoice([("a", End)])`

**Tests:** `src/mvl/checker/types.rs::tests::session_send_dual_is_receive`,
`src/mvl/checker/types.rs::tests::session_receive_dual_is_send`,
`src/mvl/checker/types.rs::tests::session_internal_choice_dual_is_external`,
`src/mvl/checker/types.rs::tests::session_is_dual_of_roundtrip`

---

### Requirement 4: Well-Formedness Checking [MUST]

The checker MUST reject session types with empty choice blocks.  A `+{}` or
`&{}` with no branches is a parse error (caught in the parser) and a checker
error (defence-in-depth).

**Implementation:** `src/mvl/checker/session.rs::check_session_well_formed`,
`src/mvl/checker/errors.rs::CheckError::SessionProtocolMismatch`

---

### Requirement 5: Error Reporting [MUST]

All session type violations MUST produce structured errors with source
location (span), human-readable description, and a Requirement 1 classification.

**Implementation:** `src/mvl/checker/errors.rs::CheckError::SessionProtocolMismatch`,
`src/mvl/checker/errors.rs::CheckError::SessionDualityMismatch`,
`src/mvl/checker/errors.rs::CheckError::SessionUnknownBranch`,
`src/mvl/checker/errors.rs::CheckError::SessionAfterEnd`

---

## Future Work

- **Protocol compliance at call sites**: verify that channel operations at
  function call sites advance the session type correctly (requires linear/typestate
  tracking — planned as a follow-on issue).
- **Multiparty session types** (Honda/Yoshida/Carbone 2008): extend to N
  participants, directly relevant to MVL's actor model.
- **Actor integration**: annotate actor behaviors with session types so the
  compiler enforces the message sequence across actor boundaries.
- **`dual` built-in**: a built-in `dual(P)` expression that computes the dual
  type at the declaration site (for explicit pairing without redundant declaration).

## References

- Honda 1993 — "Types for Dyadic Interaction" (CONCUR)
- Honda/Yoshida/Carbone 2008 — Multiparty Asynchronous Session Types
- Scribble (Yoshida 2013) — Practical protocol description language
- ADR-0001 — Eleven requirements (Req 6: linearity / session types)
- ADR-0029 — Pony reference capability adaptation (sendability foundation)
- Spec 015 — Actor Model (Phase 8 foundation)
