# crud_api

Full REST API over SQLite — demonstrates **layered config**, **pkg.http**, and **pkg.sqlite**.

---

## What this demonstrates

| Concept | Syntax | Purpose |
|---------|--------|---------|
| Composite effect | `effect AppIO > DB + Env + FileRead + ...` | Bundle effects under one name |
| Layered config | defaults → TOML → env vars → CLI args | Production config pattern |
| REST routing | `new_router()`, `route()`, `dispatch()` | pkg.http helpers |
| SQLite operations | `open()`, `execute()`, `query()` | pkg.sqlite typed queries |
| Typed values | `DbValue::Int(1)`, `DbValue::Text("Alice")` | Type-safe SQL parameters |

---

## Routes

| Method | Path | Handler |
|--------|------|---------|
| GET | `/users` | List all users (JSON array) |
| POST | `/users` | Create user `{name, email}` |
| GET | `/users/{id}` | Get user by ID |
| PUT | `/users/{id}` | Update user |
| DELETE | `/users/{id}` | Delete user |

---

## Config layering

```
1. Defaults (hardcoded)     → port=8080, db=/tmp/crud_api.db
2. config.toml              → override defaults
3. CRUD_API_* env vars      → override TOML
4. CLI args (--port, --db)  → override env
```

---

## Running

```bash
make build
cd examples/crud_api
make run

# In another terminal:
curl -s http://127.0.0.1:8080/users | jq .
curl -X POST http://127.0.0.1:8080/users \
     -H 'Content-Type: application/json' \
     -d '{"name":"Alice","email":"alice@example.com"}'
```

---

## Related

- Pattern: `.openspec/patterns/001-config.md`
- Issue: #1000
