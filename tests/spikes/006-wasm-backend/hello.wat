;; Spike 006 — WASM Backend — hand-translated WAT for `hello.mvl`
;;
;; Source (hello.mvl):
;;     fn main() -> Unit ! Console { println(add(2, 3).to_string()) }
;;
;; This variant exercises a *host import* — WASI preview 1's `fd_write` —
;; which is the mechanism the eventual `extern "wasm"` ABI would lower to.
;; We hard-code the printed string "5\n" rather than implement i64→String
;; in WAT; the runtime helper for that (`mvl_int_to_string`) hasn't been
;; ported to WASM yet. See the README for what a real emitter would do.
;;
;; Run:
;;     wasm-tools parse hello.wat -o hello.wasm
;;     wasmtime run hello.wasm    # → prints "5"

(module
  ;; ---- Host imports (the `extern "wasm"` story) ------------------------
  ;;
  ;; WASI preview 1 exposes `fd_write` for writing to file descriptors.
  ;; Signature:  fd_write(fd, iovs_ptr, iovs_len, nwritten_ptr) -> errno
  ;; All four params and the result are i32 in WASI 0.1.
  (import "wasi_snapshot_preview1" "fd_write"
    (func $fd_write (param i32 i32 i32 i32) (result i32)))

  ;; Linear memory — WASI requires the guest to expose its memory so the
  ;; host can read the iovec entries and the bytes they point at.
  (memory 1)
  (export "memory" (memory 0))

  ;; Place the string "5\n" at offset 8. Bytes 0..7 are reserved for the
  ;; iovec we build at runtime in $_start.
  (data (i32.const 8) "5\n")

  ;; ---- Entry point -----------------------------------------------------
  ;;
  ;; WASI command modules export `_start` (no params, no results) — the
  ;; runtime treats it as `main`.
  (func $_start
    ;; Build an iovec at offset 0:  struct { i32 ptr; i32 len; }
    ;;   iovec.ptr = 8     (address of the bytes "5\n")
    i32.const 0
    i32.const 8
    i32.store
    ;;   iovec.len = 2     (length of "5\n")
    i32.const 4
    i32.const 2
    i32.store

    ;; fd_write(fd=1 stdout, iovs=0, iovs_len=1, nwritten=16)
    i32.const 1     ;; fd: stdout
    i32.const 0     ;; iovs_ptr
    i32.const 1     ;; iovs_len (one iovec entry)
    i32.const 16    ;; nwritten_ptr (host writes the byte count here)
    call $fd_write
    drop)            ;; ignore the errno return for this spike

  (export "_start" (func $_start)))
