# log_analyzer

JSONL log file analyzer — demonstrates **extern trust boundaries** and **std.args** CLI parsing.

---

## What this demonstrates

| Concept | Syntax | Purpose |
|---------|--------|---------|
| Trust boundary | `extern "rust" { fn analyze_and_format(...) }` | Domain pipeline in Rust |
| Tainted input | `content: Tainted[String]` | File contents are untrusted |
| CLI parsing | `parse_args()`, `required()`, `optional()` | std.args usage |
| Structured logging | `std.log.{Logger, default_logger}` | Replaces raw println |
| Option.map | `level_opt.map(\|l\| ...)` | Idiomatic None/Some handling |

---

## Architecture

```
┌─────────────────────────────────────────────────────┐
│  main.mvl                                           │
│                                                     │
│  parse_args() ──► --file logs.jsonl                 │
│                   --level error (optional)          │
│                                                     │
│  read_file() ──► Tainted[String]                    │
│       │                                             │
│       ▼                                             │
│  analyze_and_format(content, level)                 │
│       │                                             │
│       └──► Result[String, PipelineError]            │
└─────────────────────────────────────────────────────┘
                      │
              [trust boundary]
                      │
                      ▼
┌─────────────────────────────────────────────────────┐
│  bridge.rs                                          │
│  - Parses JSONL lines                               │
│  - Filters by log level                             │
│  - Builds aggregated report                         │
└─────────────────────────────────────────────────────┘
```

---

## Usage

```bash
mvl run main.mvl -- --file logs.jsonl
mvl run main.mvl -- --file logs.jsonl --level error
```

---

## Running

```bash
make build
cd examples/log_analyzer
python3 log_generator.py > logs.jsonl  # generate sample data
make test
```

---

## Related

- stdlib: `std/args.mvl`
- stdlib: `std/log.mvl`
