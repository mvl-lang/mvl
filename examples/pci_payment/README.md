# pci_payment

PCI-DSS compliant payment card processing — demonstrates **user-defined IFC labels** for financial regulatory compliance.

---

## What this demonstrates

| Concept | Syntax | Purpose |
|---------|--------|---------|
| User-defined label | `label PCI` | Payment Card Industry data wrapper |
| Audited tokenization | `relabel tokenize: PCI -> _ audit` | Card-to-token with audit trail |
| Card ingestion | `relabel ingest_pci: _ -> PCI` | Mark card data as PCI at entry |
| Tainted input | `relabel trust(input, "CARD-VALIDATE")` | User input validated before use |
| Label composition | `Tainted -> bare -> PCI -> token` | Multi-step boundary crossing |

---

## PCI-DSS compliance guarantees (compile-time)

1. **Card data cannot reach logs**: `logger.info("card", {"n": card})` is a compile error — `PCI[String] != String`
2. **Card data cannot reach storage**: `db_insert("cards", card)` is a compile error — raw card numbers cannot persist
3. **Every tokenization is audited**: `tokenize` has declaration-level `audit` — runtime event at every call
4. **All transitions are grepable**: `grep "relabel tokenize"` shows every card-data exit in the codebase
5. **User input is tainted**: card numbers from forms require `relabel trust` before ingestion

---

## Architecture

```
  User input ──► Tainted[String]
                      │
                      ▼
              relabel trust(input, "CARD-VALIDATE")
                      │
                      ▼
                 bare String
                      │
              validate_luhn(bare)
                      │
                      ▼
              relabel ingest_pci(validated, "CARD-INGEST")
                      │
                      ▼
                 PCI[String]       ◄── compile-time wall
                      │                 Cannot reach:
                      │                 - logger.info()
                      │                 - db_insert()
                      │                 - network send
                      ▼
              relabel tokenize(card, "PAYMENT-001")
                      │                 ⚡ runtime audit event
                      ▼
                 bare String (token) ──► safe for processor, storage, logs
```

---

## Files

| File | Purpose |
|------|---------|
| `ifc.mvl` | PCI label and relabel transition declarations |
| `payment.mvl` | Card ingestion, validation, tokenization, payment processing |

---

## Running

```bash
# From the repo root:
make build
cd examples/pci_payment
make check
```

---

## Related

- Spec: `.openspec/specs/003-information-flow/spec.md`
- ADR: `.openspec/adr/0036-ifc-simplification-drop-transparent-sink.md`
- PCI-DSS Requirement 3: Protect stored cardholder data
- PCI-DSS Requirement 10: Track and monitor all access to cardholder data
