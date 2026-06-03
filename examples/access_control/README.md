# access_control

IFC-aware authentication system — demonstrates **Req 11 information flow control** with trust boundaries.

---

## What this demonstrates

| Concept | Syntax | Purpose |
|---------|--------|---------|
| Secret label | `Secret[String]` | Password hashes cannot flow to output |
| Tainted label | `Tainted[String]` | Raw user input requires validation |
| Trust boundary | `extern "rust" { fn hash_verify(...) }` | Credential validation happens in Rust |
| Relabel | `relabel trust(raw_username, "VALIDATED")` | Explicit audit point for IFC transition |
| Effect declaration | `! CryptoRandom` | Token generation uses secure randomness |

---

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│  MVL (type-checked, IFC-enforced)                           │
│                                                             │
│  raw_username: Tainted[String]  ──┐                         │
│  raw_password: Tainted[String]  ──┼──► hash_verify() ──► Result[Unit, AuthError]
│  stored_hash:  Secret[String]   ──┘         │                │
│                                             │                │
│                                      [trust boundary]        │
└─────────────────────────────────────────────────────────────┘
                                             │
                                             ▼
┌─────────────────────────────────────────────────────────────┐
│  Rust bridge (bridge.rs)                                    │
│  - Consumes Secret[String] — cannot escape                  │
│  - Validates Tainted[String] — result is plain Ok/Err       │
└─────────────────────────────────────────────────────────────┘
```

---

## IFC rules enforced

1. **Secret cannot reach output**: `println(stored_hash)` is a compile error
2. **Tainted must be validated**: raw input cannot be used directly
3. **Relabel requires audit tag**: every IFC transition is documented

---

## Running

```bash
# From the repo root:
make build
cd examples/access_control
make test
```

---

## Related

- Spec: `.openspec/specs/003-information-flow/spec.md`
- ADR: `.openspec/adr/0006-ifc-labels.md`
