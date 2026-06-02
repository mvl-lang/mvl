# Spec 020: ZMTP 3.x Wire Compatibility

> pkg.zmq ZMTP 3.x wire protocol for interop with pyzmq, zmq.rs, cppzmq.
> Issue: #1047, #1053, #1054, #1055

## Overview

The pkg.zmq package originally used simplified framing (4-byte length prefix,
one-connection-per-message). This spec adds ZMTP 3.x wire compatibility so MVL
services can interoperate with the ZeroMQ ecosystem (pyzmq, zmq.rs, cppzmq).

Supported patterns: REQ/REP, PUSH/PULL, PUB/SUB.

## Architecture

```
┌─────────────────────────────────────────────────────┐
│  MVL Application                                    │
│  (server_zmtp.mvl, server_pull.mvl, client_sub.mvl) │
├─────────────────────────────────────────────────────┤
│  pkg.zmq.reqrep    rep_serve_zmtp / req_request_zmtp│
│  pkg.zmq.pushpull  pull_serve_zmtp / push_connect   │
│  pkg.zmq.pubsub    sub_connect_zmtp / pub_serve     │
├─────────────────────────────────────────────────────┤
│  pkg.zmq.zmtp      greeting + NULL auth + framing   │
│                     PING/PONG + SUBSCRIBE commands   │
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

**Tests:** `pkg/zmq/tests/zmtp_handshake_integration.mvl`, `pkg/zmq/src/zmq_test.mvl`

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

**Implementation:** `std/net.mvl::zmtp_server`, `examples/zmq_hello/`

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

### Requirement 10: PUSH/PULL Pattern [MUST]

ZMTP 3.x PUSH/PULL one-directional message pipeline. PULL binds and
accepts PUSH connections. Messages are single frames (no envelope).

**Implementation:** `pkg/zmq/src/pushpull.mvl::pull_serve_zmtp`, `push_connect_zmtp`

#### Scenario: pyzmq PUSH to MVL PULL

- GIVEN MVL ZMTP PULL server on port 5557
- WHEN pyzmq PUSH client sends 3 messages
- THEN PULL server receives all 3 messages

**Tests:** `examples/zmq_hello/Makefile::test-pushpull`

---

### Requirement 11: PUB/SUB Pattern [MUST]

ZMTP 3.x PUB/SUB fan-out broadcast. PUB binds and accepts SUB connections.
After handshake, SUB sends SUBSCRIBE command (RFC 37, 9-byte name
"SUBSCRIBE" + topic prefix). PUB sends matching messages as single frames.

**Implementation:** `pkg/zmq/src/pubsub.mvl::sub_connect_zmtp`, `pub_serve_zmtp`

#### Scenario: pyzmq PUB to MVL SUB

- GIVEN pyzmq PUB server on port 5558 publishing 3 weather messages
- WHEN MVL SUB client connects and subscribes to all topics
- THEN SUB client receives all 3 messages

**Tests:** `examples/zmq_hello/Makefile::test-pubsub`

---

### Requirement 12: PING/PONG Heartbeat [MUST]

ZMTP 3.1 heartbeat support. When `zmtp_recv_message` encounters a PING
command frame, it automatically responds with PONG (echoing the context
bytes, omitting the 2-byte TTL) and continues receiving.

PING format: [name_len=4]["PING"][2-byte TTL][context]
PONG format: [name_len=4]["PONG"][context]

**Implementation:** `pkg/zmq/src/zmtp.mvl::handle_command`, `is_ping`, `ping_context`

#### Scenario: PING detected during message receive

- GIVEN a ZMTP connection with heartbeat-enabled peer
- WHEN peer sends PING command between message frames
- THEN server responds with PONG and continues receiving messages

---

### Requirement 13: Single-Frame Message API [MUST]

`zmtp_send_body` sends a single message frame without the REQ/REP envelope
delimiter. Used by PUSH/PULL and PUB/SUB patterns.

**Implementation:** `pkg/zmq/src/zmtp.mvl::zmtp_send_body`

---

## File Inventory

| File | Role |
|------|------|
| `std/net.mvl` | Declares `tcp_read_exact`, `tcp_shutdown_write` builtins |
| `runtime/rust/src/stdlib/net.rs` | Rust backend: read_exact, shutdown_write, Latin-1 I/O |
| `runtime/llvm/src/stdlib/net.rs` | LLVM backend: same C-ABI exports |
| `runtime/rust/src/stdlib/primitives.rs` | Latin-1 `str_from_bytes`, `str_byte_at` |
| `src/mvl/backends/llvm.rs` | LLVM codegen return type registration |
| `pkg/zmq/src/zmtp.mvl` | ZMTP 3.x greeting, NULL auth, frame codec, PING/PONG, SUBSCRIBE |
| `pkg/zmq/src/reqrep.mvl` | `rep_serve_zmtp`, `req_request_zmtp` |
| `pkg/zmq/src/pushpull.mvl` | `pull_serve_zmtp`, `push_connect_zmtp` |
| `pkg/zmq/src/pubsub.mvl` | `sub_connect_zmtp`, `pub_serve_zmtp` |
| `examples/zmq_hello/server_zmtp.mvl` | ZMTP REP server example |
| `examples/zmq_hello/server_pull.mvl` | ZMTP PULL server example |
| `examples/zmq_hello/client_sub.mvl` | ZMTP SUB client example |
| `examples/zmq_hello/client_zmq.py` | pyzmq REQ client |
| `examples/zmq_hello/client_push.py` | pyzmq PUSH client |
| `examples/zmq_hello/server_pub.py` | pyzmq PUB server |
| `examples/zmq_hello/client_rust/` | Rust zeromq crate REQ client |
| `examples/zmq_hello/Makefile` | Test orchestration for all modes |
