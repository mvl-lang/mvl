# task_pipeline

CSV data pipeline — demonstrates **Req 7 effect boundary** with pure transforms.

---

## What this demonstrates

| Concept | Syntax | Purpose |
|---------|--------|---------|
| Effect boundary | `main.mvl` = `! FileRead + Console`, transforms = pure | Testable core |
| Result chain | Nested match for `IOFailure`, `ParseFailed` | All errors handled |
| CLI args | `--input data.csv`, `--threshold 50.0` | std.args integration |
| Pure transforms | `apply_filter_by_amount()`, `apply_enrich_high_value()` | No effects |

---

## Pipeline stages

```
input.csv
    │
    ▼
read_file() ──► String
    │
    ▼
parse_csv() ──► List[Record]          (pure, in parser.mvl)
    │
    ▼
apply_filter_by_amount(threshold) ──► List[Record]   (pure)
    │
    ▼
apply_enrich_high_value() ──► List[Record]           (pure)
    │
    ▼
compute_summary() ──► Summary                        (pure)
    │
    ▼
format + println()                    (! Console)
```

---

## Effect boundary check

```bash
grep '!' examples/task_pipeline/*.mvl | grep -v '//'
# Only main.mvl and run() appear
```

---

## Usage

```bash
mvl run main.mvl -- --input data.csv
mvl run main.mvl -- --input data.csv --threshold 50.0
```

---

## Running

```bash
make build
cd examples/task_pipeline
make test
```

---

## Related

- Assurance note: 0 extern blocks
- Pattern: Pure transform pipeline
