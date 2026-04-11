# 7. Ownership and Borrowing

MVL uses ownership and borrowing to guarantee memory safety (Req 2) and resource linearity (Req 6) without a garbage collector.

## 7.1 Ownership Rules

1. Every value has exactly one *owner*
2. When the owner goes out of scope, the value is dropped (resources released)
3. Ownership can be transferred via `move`
4. After a move, the original binding is invalid

```mvl
let a = create_buffer();
let b = move a;                      // ownership transferred
// a is no longer valid — compile error if used
```

## 7.2 Borrowing

Borrowing allows temporary access without transferring ownership.

### Shared borrows (`&T`)

```mvl
fn length(s: &String) -> UInt {
    s.len()
}

let name = "Alice".to_string();
let len = length(&name);            // borrow — name still valid
```

Multiple shared borrows can coexist. Shared borrows are immutable.

### Exclusive borrows (`&mut T`)

```mvl
fn push_item(list: &mut Array<Int>, item: Int) -> () {
    list.push(item);
}

let mut items = [1, 2, 3];
push_item(&mut items, 4);
```

Only one exclusive borrow can exist at a time. No shared borrows can coexist with an exclusive borrow.

## 7.3 Borrow Rules

| Rule | Enforced at |
|------|------------|
| No use-after-move | Compile time |
| No use-after-free | Compile time |
| Borrow cannot outlive owner | Compile time |
| At most one `&mut` at a time | Compile time |
| No `&mut` while `&` exists | Compile time |
| No double-free | Compile time (ownership) |

## 7.4 Linear Resources

Types that represent external resources (files, connections, locks) have *linear* ownership — they MUST be explicitly consumed (closed, released, committed). The compiler rejects code that lets a resource go out of scope unconsumed.

```mvl
fn process_file(path: Path) -> Result<(), IOError> ! FileRead {
    let file = File.open(path)?;    // file opened — must be closed
    let content = file.read_all()?;
    file.close();                    // consumed — compiler satisfied
    Ok(())
}
// If file.close() is missing: COMPILE ERROR — linear resource not consumed
```

## 7.5 Copy Types

Small, stack-allocated types implement `Copy` and are implicitly duplicated instead of moved:

- All primitive types (`Int`, `Float64`, `Bool`, `Char`, etc.)
- Tuples of copy types
- User types can opt in: `type Point = struct { ... } derives Copy`

Non-copy types (strings, collections, resources) must be explicitly moved or borrowed.
