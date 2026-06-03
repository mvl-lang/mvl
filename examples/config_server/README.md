# config_server

HTTP config server — demonstrates **Req 7 effects**, **Req 10 refinements**, and **Req 11 IFC** together.

---

## What this demonstrates

| Concept | Syntax | Purpose |
|---------|--------|---------|
| Refinement types | `type Port = Int where self > 0 && self <= 65535` | Invalid ports unrepresentable |
| Secret handling | `api_key: Secret[String]` | API key cannot leak to logs |
| Effect declaration | `! FileRead + Net + Log + Console` | All I/O declared |
| Pure handlers | `handler.mvl` — no `!` | Request logic is testable |
| Trust boundary | `extern "rust" { ... }` | Config loading + server I/O |

---

## Architecture

```
┌─────────────────────────────────────────────────────┐
│  main.mvl (! FileRead + Net + Log + Console)        │
│                                                     │
│  load_config() ──► Config { port: Port,             │
│                             api_key: Secret[String] │
│                             ... }                   │
│                                                     │
│  server_recv() ──► Request ──► handle() ──► Response│
│                        │           │                │
│                   [pure handler.mvl]                │
└─────────────────────────────────────────────────────┘
```

---

## Refinement guarantees

| Type | Constraint | Compile-time check |
|------|------------|-------------------|
| `Port` | `1 ≤ x ≤ 65535` | `load_config()` returns `Err(InvalidPort)` if violated |
| `Timeout` | `0 ≤ x ≤ 300` | Config validation |
| `MaxConns` | `1 ≤ x ≤ 10000` | Config validation |

---

## Running

```bash
make build
cd examples/config_server
make test
```

---

## Related

- Spec: `.openspec/specs/018-refinement-solver/spec.md`
- Spec: `.openspec/specs/003-information-flow/spec.md`
