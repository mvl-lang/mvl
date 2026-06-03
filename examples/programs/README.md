# programs

Small standalone examples — one file each, demonstrating individual MVL features.

---

## Index

| File | Demonstrates | Requirements |
|------|--------------|--------------|
| `hello_world.mvl` | Minimal program | Req 1, 7 |
| `hello_mvl.mvl` | Enum match, total fn | Req 1, 3 |
| `calculator.mvl` | Arithmetic, if/else | Req 1, 3, 8 |
| `shapes.mvl` | ADTs, multi-enum match | Req 1, 3 |

### Types & Ownership

| File | Demonstrates | Requirements |
|------|--------------|--------------|
| `core_types_demo.mvl` | Int, Float, String, Bool | Req 1 |
| `collections_basic.mvl` | List, Map, Set | Req 1 |
| `box_field_deref.mvl` | Box ownership | Req 2, 6 |
| `linked_list.mvl` | Recursive enum, Box | Req 2, 6, 8 |
| `struct_value_semantics.mvl` | Clone-on-pass | Req 6 |

### Functions

| File | Demonstrates | Requirements |
|------|--------------|--------------|
| `generic_fns.mvl` | Generic constraints | Req 1, 9 |
| `hof_lambdas.mvl` | Higher-order functions | Req 1 |
| `closure_lambdas.mvl` | Closures, captures | Req 1 |
| `else_if_chain.mvl` | Control flow | Req 1 |

### IFC & Security

| File | Demonstrates | Requirements |
|------|--------------|--------------|
| `auth_handler.mvl` | Result + IFC labels | Req 5, 11 |
| `password_checker.mvl` | Taint tracking | Req 11 |
| `safe_division.mvl` | Refinement + IFC | Req 5, 10, 11 |

### Actors

| File | Demonstrates | Requirements |
|------|--------------|--------------|
| `actor_spawn.mvl` | Actor creation | Req 9 |
| `actor_send.mvl` | Message passing | Req 9 |

### Misc

| File | Demonstrates | Requirements |
|------|--------------|--------------|
| `println_non_string_first_arg.mvl` | println type safety | Req 1, 7 |
| `bridge_ok/` | extern Rust interop | Req 7 |
| `random_dice/` | Random effect | Req 7 |

---

## Running

```bash
make build
mvl run examples/programs/hello_world.mvl
mvl run examples/programs/calculator.mvl
# etc.
```

---

## Related

- These programs are used by `tests/cross_backend.rs` for parity testing
- Corpus tests in `tests/corpus/` also reference these patterns
