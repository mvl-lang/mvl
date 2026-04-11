# 4. Statements

## 4.1 Let Bindings

```mvl
let x: Int = 42;                    // immutable (default)
let mut count: Int = 0;             // mutable (explicit)
```

All bindings are immutable by default ([Req 6](../requirements.md#req-6)). The `mut` keyword opts into mutability.

Type annotation is required. No type inference on bindings — the LLM generates the type, the compiler verifies it.

### Destructuring

```mvl
let (x, y) = get_coordinates();
let Point { x, y } = origin;
let Some(user) = find_user(id);     // COMPILE ERROR: non-exhaustive
```

## 4.2 Assignment

```mvl
count = count + 1;                  // only on `mut` bindings
point.x = 3.0;                      // only on `mut` fields
```

Assigning to an immutable binding is a compile error.

## 4.3 If / Else

```mvl
if condition {
    do_something();
} else if other_condition {
    do_other();
} else {
    do_fallback();
}
```

`if` is also an expression (see [Chapter 5](05-expressions.md)).

## 4.4 Match

```mvl
match shape {
    Circle(r) => area_circle(r),
    Rect(w, h) => w * h,
    Triangle(a, b, c) => heron(a, b, c),
}
```

Match MUST be exhaustive ([Req 3](../requirements.md#req-3)). The compiler rejects if any variant is unhandled. See [Chapter 6: Pattern Matching](06-patterns.md).

## 4.5 For Loop

```mvl
for item in collection {
    process(item);
}

for i in 0..10 {
    println(i.to_string());
}
```

`for` iterates over anything implementing the `Iterator` trait. It is bounded — the iterator must be finite. For unbounded iteration, use `while` in a `partial` function.

## 4.6 While Loop

```mvl
partial fn event_loop() -> Never ! Console {
    while running {
        let event = poll();
        handle(event);
    }
}
```

`while` is only permitted in `partial` functions ([Req 8](../requirements.md#req-8)). In `total` functions, use `for` with a finite iterator.

## 4.7 Return

```mvl
fn find(items: Array<Int>, target: Int) -> Option<UInt> {
    for (i, item) in items.enumerate() {
        if item == target {
            return Some(i);          // early return
        }
    }
    None                             // implicit return (last expression)
}
```

The last expression in a block is its return value. `return` is for early exits only.

## 4.8 Expression Statements

Any expression followed by `;` is a statement. The value is discarded.

```mvl
do_something();                      // function call as statement
map.insert("key", value);           // method call as statement
```
