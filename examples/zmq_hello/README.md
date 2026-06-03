# zmq_hello

ZMTP 3.x wire protocol — demonstrates **pkg.zmq** pure-MVL ZeroMQ implementation.

---

## What this demonstrates

| Concept | Syntax | Purpose |
|---------|--------|---------|
| Wire protocol | `zmtp_handshake_client()`, `zmtp_recv_message()` | ZMTP 3.x in pure MVL |
| Socket types | `ZmtpSocketType::Sub`, `ZmtpSocketType::Pull` | ZeroMQ patterns |
| TCP networking | `tcp_connect()`, `tcp_read_request()` | std.net usage |
| Cross-language | Python/Rust servers, MVL clients | Wire compatibility |

---

## Architecture

MVL implements the ZMTP 3.x wire protocol directly — no libzmq dependency.

```
┌─────────────────────────────────────────┐
│  pyzmq / zmq.rs / cppzmq server         │
│  (any ZMTP 3.x implementation)          │
└─────────────────────────────────────────┘
                    │
            [ZMTP 3.x wire protocol]
                    │
                    ▼
┌─────────────────────────────────────────┐
│  MVL client (client_sub.mvl)            │
│                                         │
│  tcp_connect() ──► TcpStream            │
│  zmtp_handshake_client() ──► Ok         │
│  zmtp_send_subscribe("") ──► Ok         │
│  loop: zmtp_recv_message() ──► String   │
└─────────────────────────────────────────┘
```

---

## Files

| File | Purpose |
|------|---------|
| `client_sub.mvl` | SUB client — receives from PUB server |
| `server_pub.py` | PUB server (pyzmq) |
| `client_push.py` | PUSH client (pyzmq) for testing |
| `client_zmq.py` | Generic ZMQ client (pyzmq) |
| `client_rust/` | Rust ZMQ client for comparison |

---

## Running

```bash
# Terminal 1: Start Python PUB server
python3 server_pub.py

# Terminal 2: Run MVL SUB client
make build
cd examples/zmq_hello
make run-sub
```

---

## Related

- Spec: `.openspec/specs/020-zmtp-wire/spec.md`
- Package: `pkg/zmq`
