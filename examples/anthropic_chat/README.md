# anthropic_chat

Claude API client — demonstrates **pkg.anthropic** with full IFC tracking.

---

## What this demonstrates

| Concept | Syntax | Purpose |
|---------|--------|---------|
| Secret API key | `api_key: Secret[String]` | Key cannot leak to logs |
| Tainted response | `resp.content: Tainted[String]` | API response is untrusted |
| Relabel trust | `relabel trust(resp.content, "DISPLAY-OUTPUT")` | Explicit audit point |
| Effect declaration | `! Net` | Network access required |
| Typed SDK | `Claude::messages()`, `ModelId::Sonnet4_6` | Type-safe API calls |

---

## IFC flow

```
┌─────────────────────────────────────────────────────┐
│  Environment                                        │
│  ANTHROPIC_API_KEY ──► get_secret() ──► Secret[String]
└─────────────────────────────────────────────────────┘
                              │
                    [Secret — cannot reach println]
                              │
                              ▼
┌─────────────────────────────────────────────────────┐
│  ask_claude(api_key, question) ! Net                │
│                                                     │
│  Claude::new(api_key)                               │
│  client.messages(...) ──► Response                  │
│                              │                      │
│              [Tainted — API response is untrusted]  │
└─────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────┐
│  display_response(resp) ! Console                   │
│                                                     │
│  relabel trust(resp.content, "DISPLAY-OUTPUT")     │
│  relabel trust(resp.model, "DISPLAY-META")         │
│  println(...)                                       │
└─────────────────────────────────────────────────────┘
```

---

## The function signature IS the threat model

```mvl
fn ask_claude(...) -> Result[Response, AnthropicError] ! Net
fn display_response(...) -> Unit ! Console
```

- `ask_claude` needs network access — declared via `! Net`
- `display_response` writes to console — declared via `! Console`
- API key stays `Secret` — cannot reach `println`
- Response is `Tainted` — must `relabel trust` to use

---

## Running

```bash
export ANTHROPIC_API_KEY=sk-ant-...
make build
cd examples/anthropic_chat
make run
```

---

## Related

- Package: `pkg/anthropic`
- Spec: `.openspec/specs/003-information-flow/spec.md`
