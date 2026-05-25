#!/usr/bin/env python3
# examples/zmq_hello/client.py — REQ client for the MVL REP server.
#
# Uses raw sockets with the pkg.zmq wire format: 4-byte big-endian length prefix.
# No pyzmq or libzmq needed — the protocol is our own simplified framing.
#
# Usage:
#   cargo run -- run examples/zmq_hello/server.mvl &
#   python3 examples/zmq_hello/client.py

import socket
import struct
import sys


def send_msg(sock: socket.socket, msg: str) -> None:
    data = msg.encode()
    sock.sendall(struct.pack(">I", len(data)) + data)


MAX_MSG = 64 * 1024 * 1024  # 64 MB — matches pkg.zmq decode_frame limit


def recv_msg(sock: socket.socket) -> str:
    header = b""
    while len(header) < 4:
        chunk = sock.recv(4 - len(header))
        if not chunk:
            raise ConnectionError("server closed connection before sending length header")
        header += chunk
    length = struct.unpack(">I", header)[0]
    if length > MAX_MSG:
        raise ValueError(f"message too large: {length} bytes (max {MAX_MSG})")
    body = b""
    while len(body) < length:
        chunk = sock.recv(length - len(body))
        if not chunk:
            raise ConnectionError("server closed connection before sending full body")
        body += chunk
    return body.decode()


messages = ["hello from Python", "the quick brown fox", "MVL + Python works"]

for msg in messages:
    with socket.create_connection(("127.0.0.1", 5555)) as s:
        send_msg(s, msg)
        reply = recv_msg(s)
    print(f"  sent: {msg!r}")
    print(f" reply: {reply!r}")
    print()
