# Spec 020: ZMTP 3.x Wire Compatibility

> pkg.zmq ZMTP 3.x wire protocol for interop with pyzmq, zmq.rs, cppzmq.
> Issue: #1047

## Overview

The pkg.zmq package originally used simplified framing (4-byte length prefix,
one-connection-per-message). This spec adds ZMTP 3.x wire compatibility so MVL
services can interoperate with the ZeroMQ ecosystem (pyzmq, zmq.rs, cppzmq).

## Architecture

```
┌─────────────────────────────────────────────────────┐
│  MVL Application (server_zmtp.mvl)                  │
├─────────────────────────────────────────────────────┤
│  pkg.zmq.reqrep    rep_serve_zmtp / req_request_zmtp│
├─────────────────────────────────────────────────────┤
│  pkg.zmq.zmtp      greeting + NULL auth + framing   │
├─────────────────────────────────────────────────────┤
│  std.net            tcp_read_exact / tcp_write       │
├─────────────────────────────────────────────────────┤
│  mvl_runtime        Latin-1 binary I/O              │
└─────────────────────────────────────────────────────┘
```

---

### Requirement 1: ZMTP 3.x Greeting Exchange [MUST]

Both peers exchange a 64-byte greeting on connection. The greeting contains
the protocol signature (0xFF...0x7F), version (3.1), security mechanism
(NULL), and as-server flag.

**Implementation:** `pkg/zmq/src/zmtp.mvl::make_greeting`, `validate_greeting`

#### Scenario: Server sends and validates greeting

- GIVEN a ZMTP REP server on port 5556
- WHEN a pyzmq REQ client connects
- THEN both peers exchange valid 64-byte greetings with NULL mechanism

**Tests:** `examples/zmq_hello/Makefile::test-zmq`

---

### Requirement 2: NULL Auth Handshake [MUST]

After greeting, both peers exchange READY command frames containing
Socket-Type metadata. The READY command uses the ZMTP command frame
format (flag byte 0x04 + size + body with property key-value pairs).

**Implementation:** `pkg/zmq/src/zmtp.mvl::make_ready_body`, `parse_ready_body`

#### Scenario: Socket type negotiation

- GIVEN a ZMTP REP server
- WHEN a pyzmq REQ client performs handshake
- THEN server sends Socket-Type=REP, client sends Socket-Type=REQ

**Tests:** `examples/zmq_hello/Makefile::test-zmq`

---

### Requirement 3: ZMTP Frame Codec [MUST]

Frame format: [flags: 1 byte][size: 1 or 8 bytes][body: N bytes].
Flag bits: MORE (0x01), LONG (0x02), COMMAND (0x04).
Short frames (body < 256 bytes) use 1-byte size.
Long frames use 8-byte big-endian size.

**Implementation:** `pkg/zmq/src/zmtp.mvl::read_frame_raw`, `write_frame_raw`

#### Scenario: Short frame round-trip

- GIVEN a ZMTP connection after handshake
- WHEN client sends "hello" (5 bytes)
- THEN server reads frame with flags=0x00, size=5, body="hello"

#### Scenario: Multi-frame envelope

- GIVEN a ZMTP REQ/REP connection
- WHEN client sends a message
- THEN message is sent as [empty delimiter, MORE=1] + [body, MORE=0]

---

### Requirement 4: Multi-Frame Message Support [MUST]

ZMTP REQ/REP messages use an envelope: empty delimiter frame (MORE=1)
followed by body frame (MORE=0). The `zmtp_recv_message` function skips
delimiter and command frames, returning only the body.

**Implementation:** `pkg/zmq/src/zmtp.mvl::zmtp_recv_message`, `zmtp_send_message`

#### Scenario: pyzmq client round-trip

- GIVEN MVL ZMTP server running on port 5556
- WHEN pyzmq REQ client sends 3 messages
- THEN all 3 replies match "echo: <original>"

**Tests:** `examples/zmq_hello/Makefile::test-zmq`

---

### Requirement 5: Backward Compatibility [MUST]

The simplified framing mode (4-byte length prefix, connection-per-message)
remains available via `rep_serve` and `req_request`. New ZMTP functions
are `rep_serve_zmtp` and `req_request_zmtp`.

**Implementation:** `pkg/zmq/src/reqrep.mvl`

#### Scenario: Simplified mode still works

- GIVEN MVL server on port 5555 using `rep_serve`
- WHEN raw Python client sends 3 messages
- THEN all 3 replies match "echo: <original>"

**Tests:** `examples/zmq_hello/Makefile::test`

---

### Requirement 6: Binary-Safe I/O [MUST]

Network I/O uses Latin-1 encoding (each byte maps to Unicode codepoint
0–255) to preserve binary data. This enables ZMTP's 0xFF greeting byte
and arbitrary binary frame content.

**Implementation:** `runtime/rust/src/stdlib/net.rs`, `runtime/rust/src/stdlib/primitives.rs`

#### Scenario: 0xFF byte round-trips through tcp_read_exact + tcp_write

- GIVEN `String::from_bytes([from_int(255)])`
- WHEN written via `tcp_write` and read via `tcp_read_exact`
- THEN `byte_at(0).to_int()` returns 255

---

### Requirement 7: tcp_read_exact Primitive [MUST]

New stdlib builtin `tcp_read_exact(stream, n)` reads exactly N bytes
from a persistent connection without waiting for EOF.

**Implementation:** `std/net.mvl::tcp_read_exact`, `runtime/rust/src/stdlib/net.rs::_tcp_read_exact`

---

### Requirement 8: Cross-Language Interop [SHOULD]

The ZMTP implementation should be validated against multiple ZMQ
client libraries to confirm wire compatibility.

#### Scenario: pyzmq (Python) interop

- GIVEN MVL ZMTP server
- WHEN pyzmq 27.x REQ client sends messages
- THEN all round-trips succeed

**Tests:** `examples/zmq_hello/client_zmq.py`

#### Scenario: zeromq crate (Rust) interop

- GIVEN MVL ZMTP server
- WHEN zeromq 0.4.x REQ client sends messages
- THEN all round-trips succeed

**Tests:** `examples/zmq_hello/client_rust/`

---

### Requirement 9: IFC Labels Preserved [MUST]

All received ZMTP message bodies are `Tainted[String]`. The handler
must `relabel trust` with a context-specific audit tag before use.
Protocol parsing uses the `ZMTP-PARSE` audit tag.

**Implementation:** `pkg/zmq/src/zmtp.mvl::read_bytes`, `zmtp_recv_message`

---

## File Inventory

| File | Role |
|------|------|
| `std/net.mvl` | Declares `tcp_read_exact`, `tcp_shutdown_write` builtins |
| `runtime/rust/src/stdlib/net.rs` | Rust backend: read_exact, shutdown_write, Latin-1 I/O |
| `runtime/llvm/src/stdlib/net.rs` | LLVM backend: same C-ABI exports |
| `runtime/rust/src/stdlib/primitives.rs` | Latin-1 `str_from_bytes`, `str_byte_at` |
| `src/mvl/backends/llvm.rs` | LLVM codegen return type registration |
| `pkg/zmq/src/zmtp.mvl` | ZMTP 3.x greeting, NULL auth, frame codec |
| `pkg/zmq/src/reqrep.mvl` | `rep_serve_zmtp`, `req_request_zmtp` |
| `examples/zmq_hello/server_zmtp.mvl` | ZMTP REP server example |
| `examples/zmq_hello/client_zmq.py` | pyzmq REQ client |
| `examples/zmq_hello/client_rust/` | Rust zeromq crate REQ client |
| `examples/zmq_hello/Makefile` | Test orchestration for all modes |
