# hipaa_healthcare

HIPAA-compliant patient data handling — demonstrates **user-defined IFC labels** for regulated healthcare domains.

---

## What this demonstrates

| Concept | Syntax | Purpose |
|---------|--------|---------|
| User-defined label | `label PHI` | Protected Health Information wrapper |
| Audited release | `relabel hipaa_release: PHI -> _ audit` | Every PHI release emits audit event |
| Ingestion boundary | `relabel ingest_phi: _ -> PHI` | Mark raw data as PHI at entry |
| Tainted DB data | `relabel taint(row, "DB-QUERY")` | DB query results are untrusted |
| Secret credentials | `relabel classify(key, "LOAD")` | Credential isolation in storage |
| Two-step crossing | `Tainted -> bare -> PHI` | Validate then classify |

---

## HIPAA compliance guarantees (compile-time)

1. **PHI cannot reach output**: `println(phi_record)` is a compile error — `PHI[String] != String`
2. **PHI cannot reach logs**: `logger.info("x", {"name": phi_record})` is a compile error
3. **Every release is audited**: `hipaa_release` has declaration-level `audit` — runtime event at every call
4. **All transitions are grepable**: `grep "relabel hipaa_release"` shows every PHI release in the codebase
5. **DB data is tainted**: raw query results require explicit `relabel trust` before use

---

## Architecture

```
                        ┌─────────────────────────────────────┐
  Raw String ──────────►│  relabel ingest_phi(...)            │──► PHI[String]
                        │  audit tag: "PATIENT-LOAD"          │
                        └─────────────────────────────────────┘
                                                                     │
                                                                     │ (compile-time wall)
                                                                     │
                                                              Cannot reach:
                                                              - println()
                                                              - logger.info()
                                                              - network send
                                                                     │
                        ┌─────────────────────────────────────┐      │
  bare String ◄─────────│  relabel hipaa_release(...)         │◄─────┘
                        │  audit tag: "INSURER-SHARE-001"     │
                        │  ⚡ runtime audit event emitted     │
                        └─────────────────────────────────────┘

  DB query ──► Tainted[String] ──► relabel trust ──► String ──► relabel ingest_phi ──► PHI[String]
```

---

## Files

| File | Purpose |
|------|---------|
| `ifc.mvl` | PHI label and relabel transition declarations |
| `patient.mvl` | Patient data ingestion, authorized release, DB patterns |
| `db.mvl` | Generic database IFC patterns: Tainted queries, Secret storage |

---

## Running

```bash
# From the repo root:
make build
cd examples/hipaa_healthcare
make check
```

---

## Related

- Spec: `.openspec/specs/003-information-flow/spec.md`
- ADR: `.openspec/adr/0036-ifc-simplification-drop-transparent-sink.md`
- HIPAA: 45 CFR 164.312 (access controls, audit controls)
