# Runtime — Claude Code Instructions

## LLVM Backend: C-ABI Naming Convention

When emitting LLVM IR for C-ABI calls (e.g., runtime builtins), use the **unprefixed** form in IR:

```llvm
// CORRECT — LLVM IR C-ABI function calls
call void @mvl_yield_check()
call void @mvl_actor_spawn(...)
call void @mvl_string_drop(ptr %s)
call ptr @mvl_array_slice(...)
declare void @mvl_yield_check()
```

The C compiler (Clang/GCC) automatically adds platform-specific prefixes when generating symbols:
- **macOS/Darwin**: `_mvl_yield_check` (one underscore prefix)
- **Linux**: `mvl_yield_check` (no prefix)

Never hardcode the underscore in LLVM IR — the platform convention handles it transparently.
This applies to all C-ABI runtime functions in `runtime/llvm/` and `runtime/rust/`.
