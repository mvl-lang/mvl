# log_to_file

Structured logging to file — demonstrates **Req 10 refinements**, **Req 11 IFC**, and **std.log**.

---

## What this demonstrates

| Concept | Syntax | Purpose |
|---------|--------|---------|
| Refined path | `type LogPath = String where len(self) > 0` | Non-empty path guaranteed |
| Tainted env | `get() → Option[Tainted[String]]` | Env vars are untrusted |
| Relabel trust | `relabel trust(raw, "LOG-PATH-001")` | Explicit IFC boundary |
| File logger | `file_logger(fd, format, level)` | std.log to file descriptor |
| Effect bundle | `! Log` subsumes `Clock + Console` | Structured effect hierarchy |

---

## IFC + Refinement flow

```
MVL_LOG_FILE env var (optional)
        │
        ▼
get("MVL_LOG_FILE") ──► Option[Tainted[String]]
        │
        │ if Some:
        ▼
relabel trust(raw, "LOG-PATH-001") ──► String
        │
        ▼
validate_log_path(p: String where len(p) > 0) ──► LogPath
        │                    │
        │                    └── compile-time proof for literals
        │                        runtime check for env vars
        ▼
open(path) ──► Fd ──► file_logger(fd)
```

---

## Usage

```bash
# Use default path (app.log)
mvl run main.mvl

# Override via env var
MVL_LOG_FILE=/var/log/app.log mvl run main.mvl
```

---

## Running

```bash
make build
cd examples/log_to_file
make test
```

---

## Related

- stdlib: `std/log.mvl`
- Spec: `.openspec/specs/018-refinement-solver/spec.md`
