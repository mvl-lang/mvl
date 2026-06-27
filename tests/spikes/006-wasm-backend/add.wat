;; Spike 006 — WASM Backend — hand-translated WAT for `add.mvl`
;;
;; Source (add.mvl):
;;     fn add(a: Int, b: Int) -> Int { a + b }
;;     fn main() -> Int { add(2, 3) }
;;
;; MVL Int → WASM i64 (matches the LLVM runtime: runtime/llvm/src/lib.rs).
;;
;; Run:
;;     wasm-tools parse add.wat -o add.wasm
;;     wasmtime run --invoke main add.wasm   # → 5
;;     wasmtime run --invoke add add.wasm 7 35  # → 42

(module
  ;; --- fn add(a: Int, b: Int) -> Int { a + b } -------------------------
  ;;
  ;; Parameters live in WASM locals (indexed). Function body in MVL is
  ;; a single expression `a + b`; that's the function's return value, so
  ;; WAT just leaves `i64.add`'s result on the stack — no `return` needed.
  (func $add (param $a i64) (param $b i64) (result i64)
    local.get $a
    local.get $b
    i64.add)

  ;; --- fn main() -> Int { add(2, 3) } -----------------------------------
  ;;
  ;; Push 2, push 3, call $add. WASM's calling convention pops the args
  ;; from the stack and pushes the result back.
  (func $main (result i64)
    i64.const 2
    i64.const 3
    call $add)

  ;; Exports — make both callable from the host (wasmtime --invoke).
  (export "add"  (func $add))
  (export "main" (func $main)))
