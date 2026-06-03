# csv_transactions

CSV parsing with IFC taint tracking — demonstrates **std.csv** and **Req 11 trust boundaries**.

---

## What this demonstrates

| Concept | Syntax | Purpose |
|---------|--------|---------|
| Tainted input | `read_file() → Tainted[String]` | File contents are untrusted |
| Decode function | `fn decode_transaction(row: List[Tainted[String]])` | Validate + relabel |
| Trust boundary | `relabel trust(cell, "TX-VALIDATED")` | Explicit audit point |
| Pure CSV API | `parse_with_headers()`, `encode_with_headers()` | No I/O in core logic |

---

## IFC flow

```
sample.csv (external file)
        │
        ▼
read_file() ──► Tainted[String]
        │
        ▼
parse_with_headers() ──► CsvWithHeaders { rows: List[List[Tainted[String]]] }
        │
        ▼
decode_transaction() ──► Result[Transaction, CsvError]
        │                       │
        │               relabel trust(cell, "TX-VALIDATED")
        │                       │
        ▼                       ▼
clean Transaction { date: String, description: String, amount_cents: Int }
```

---

## Running

```bash
make build
cd examples/csv_transactions
make test

# Or run directly:
mvl run main.mvl -- --input sample.csv
```

---

## Related

- Spec: `.openspec/specs/003-information-flow/spec.md`
- stdlib: `std/csv.mvl`
