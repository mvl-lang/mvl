# Changelog

All notable changes to pkg-zmq will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.1.0] - 2026-05-31

### Added
- Initial release -- ZeroMQ-style messaging for MVL
- REQ/REP pattern (`pkg.zmq.reqrep`): `req_send`, `rep_serve`
- PUB/SUB pattern (`pkg.zmq.pubsub`): `pub_send`, `sub_serve` with topic filtering
- PUSH/PULL pattern (`pkg.zmq.pushpull`): `push_send`, `puller_serve` for work distribution
- Simplified ZMTP wire protocol: 4-byte big-endian length prefix + body
- `encode_frame` / `decode_frame` -- pure functions, no I/O
- 64 MB frame size limit (DoS protection)
- `ZmqAddr` -- refined type: host non-empty, port 0..65535
- `parse_zmq_addr` -- parse `"tcp://host:port"`, `"tcp://*:port"`, `"host:port"`
- `zmq_bind` -- parse + TCP listen in one step
- `send_one` -- connect-write-close for fire-and-forget semantics
- `is_transient_accept_error` -- retry logic for accept loops
- IFC: all received bytes `Tainted[String]`; detainted at `ZMQ-FRAME-PARSE`, body re-tainted `ZMQ-RECV`
- ZMTP 3.x wire compatibility: PING/PONG heartbeat, interop with pyzmq/libzmq
- Pure MVL -- no `bridge.rs`, no `extern` blocks
