# pkg-zmq

ZeroMQ-style messaging patterns for [MVL](https://github.com/LAB271/mvl_language).

Pure MVL implementation of REQ/REP, PUB/SUB, and PUSH/PULL over TCP. No `libzmq` dependency. Wire protocol: simplified ZMTP (4-byte big-endian length prefix + body).

## Install

```bash
mvl add github.com/mvl-lang/pkg-zmq v0.1.0
mvl install
```

## Usage

### REQ/REP

```mvl
use pkg.zmq.reqrep.{rep_serve, req_send}

// Server
actor EchoServer {
    pub fn handle(val msg: String) { }
}

partial fn server_main() -> Unit ! Net {
    rep_serve("tcp://*:5555", EchoServer { })
}

// Client
partial fn client_main() -> Result[String, ZmqError] ! Net {
    req_send("tcp://127.0.0.1:5555", "hello")
}
```

### PUB/SUB

```mvl
use pkg.zmq.pubsub.{pub_send, sub_serve}

partial fn publisher() -> Result[Unit, ZmqError] ! Net {
    pub_send("tcp://127.0.0.1:5556", "news", "breaking: MVL ships")
}
```

### PUSH/PULL

```mvl
use pkg.zmq.pushpull.{push_send, puller_serve}

partial fn worker() -> Unit ! Net {
    puller_serve("tcp://*:5557", process_job)
}
```

## API

### Core (`pkg.zmq`)

| Item | Description |
|------|-------------|
| `ZmqError` | `NetError(String)`, `FrameError(String)`, `InvalidAddr(String)` |
| `ZmqAddr` | `{ host: String, port: Int }` (refined: host non-empty, port 0..65535) |
| `encode_frame` | Pure: encode message as 4-byte length-prefixed frame |
| `decode_frame` | Pure: decode frame, returns `Tainted[String]` |
| `parse_zmq_addr` | Parse `"tcp://host:port"` or `"host:port"` |
| `zmq_bind` | Parse address + TCP listen in one step |
| `send_one` | Connect, write pre-framed payload, close |
| `is_transient_accept_error` | True for `ConnectionReset`, `Timeout` |
| `zmq_error_msg` | Human-readable error string |

### Wire Protocol

Simplified ZMTP: `[4 bytes big-endian u32 length][N bytes body]`

- `encode_frame` / `decode_frame` are pure (no effects, no I/O)
- Maximum frame body: 64 MB (DoS protection -- larger frames rejected)

## Security

### IFC Model

All received bytes are `Tainted[String]`. Use `relabel` with an explicit audit tag after validating message content.

Frame parsing uses `ZMQ-FRAME-PARSE` (structural parsing only). The decoded body is re-tainted `ZMQ-RECV`. Applications must explicitly trust content after validation.

## License

Apache License 2.0 -- see [LICENSE](LICENSE).
