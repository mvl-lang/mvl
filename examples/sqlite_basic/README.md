# sqlite_basic

SQLite basics — demonstrates **pkg.sqlite** with typed parameters and error handling.

---

## What this demonstrates

| Concept | Syntax | Purpose |
|---------|--------|---------|
| In-memory DB | `open(":memory:")` | No file needed for demo |
| Typed values | `DbValue::Int(1)`, `DbValue::Text("Alice")` | Type-safe SQL params |
| Error matching | `match result { Err(SqliteError::InvalidSql(msg)) => ... }` | Named error variants |
| Row iteration | `while let Some(row) = ...` | Process query results |
| Partial fn | `partial fn main()` | Iteration uses `while` |

---

## Operations demonstrated

```mvl
// Create table
execute(db, "CREATE TABLE users ...", [])

// Insert with typed params
execute(db, "INSERT INTO users VALUES (?, ?, ?)",
        [DbValue::Int(1), DbValue::Text("Alice"), DbValue::Int(30)])

// Query all rows
query(db, "SELECT * FROM users WHERE age > ?", [DbValue::Int(25)])

// Query scalar
query_scalar(db, "SELECT COUNT(*) FROM users", [])
```

---

## Running

```bash
make build
cd examples/sqlite_basic
make test
```

---

## Related

- Package: `pkg/sqlite`
- stdlib: `std/db.mvl` (DbValue type)
