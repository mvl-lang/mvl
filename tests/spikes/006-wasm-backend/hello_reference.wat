;; Spike 006 — WASM Backend — hand-translated WAT for `hello.mvl`
;;
;; Source (hello.mvl):
;;     fn main() -> Unit ! Console { println("hello, world") }
;;
;; Target shape spec — this is what a `TIR → WASM` emitter should produce
;; for the classic "hello, world". The emitter's actual output in the
;; spike build directory should be behaviourally equivalent.
;;
;; Host-import story: WASI preview 1's `fd_write`. This is the mechanism
;; the eventual `extern "wasm"` ABI (see the epic) would lower to for the
;; `! Console` effect.
;;
;; Memory layout (matches what the emitter uses):
;;   0..8   iovec[0]  {ptr, len}  → points at "hello, world"
;;   8..16  iovec[1]  {ptr, len}  → points at "\n"
;;   16..20 nwritten output slot
;;   20     "\n" byte
;;   32..44 "hello, world"
;;
;; Run:
;;     wasm-tools parse hello_reference.wat -o hello.wasm
;;     wasmtime run hello.wasm    # → prints "hello, world"

(module
  ;; ---- Host imports (the `extern "wasm"` story) ------------------------
  ;;
  ;; Signature:  fd_write(fd, iovs_ptr, iovs_len, nwritten_ptr) -> errno
  ;; All four params and the result are i32 in WASI 0.1.
  (import "wasi_snapshot_preview1" "fd_write"
    (func $fd_write (param i32 i32 i32 i32) (result i32)))

  ;; Linear memory — WASI requires the guest to expose its memory so the
  ;; host can read the iovec entries and the bytes they point at.
  (memory 1)
  (export "memory" (memory 0))

  ;; Static bytes: newline at offset 20, "hello, world" at offset 32.
  (data (i32.const 20) "\n")
  (data (i32.const 32) "hello, world")

  ;; ---- Entry point -----------------------------------------------------
  ;;
  ;; WASI command modules export `_start` (no params, no results) — the
  ;; runtime treats it as `main`.
  (func $_start
    ;; iovec[0] = { ptr = 32, len = 12 }  — "hello, world"
    i32.const 0
    i32.const 32
    i32.store
    i32.const 4
    i32.const 12
    i32.store
    ;; iovec[1] = { ptr = 20, len = 1  }  — "\n"
    i32.const 8
    i32.const 20
    i32.store
    i32.const 12
    i32.const 1
    i32.store

    ;; fd_write(fd=1 stdout, iovs=0, iovs_len=2, nwritten=16)
    i32.const 1
    i32.const 0
    i32.const 2
    i32.const 16
    call $fd_write
    drop)

  (export "_start" (func $_start)))
