#!/usr/bin/env python3
# examples/zmq_hello/client_zmq.py — pyzmq REQ client for the MVL ZMTP server.
#
# Uses the standard pyzmq library (ZMTP 3.x wire protocol).
# Talks to the MVL server_zmtp.mvl on port 5556.
#
# Install: pip install pyzmq  (or: uv pip install pyzmq)
#
# Usage:
#   cargo run -- run examples/zmq_hello/server_zmtp.mvl &
#   python3 examples/zmq_hello/client_zmq.py

import sys

import zmq

ENDPOINT = "tcp://127.0.0.1:5556"

messages = ["hello from pyzmq", "the quick brown fox", "MVL + pyzmq works"]

ctx = zmq.Context()
sock = ctx.socket(zmq.REQ)
sock.connect(ENDPOINT)

for msg in messages:
    sock.send_string(msg)
    reply = sock.recv_string()
    print(f"  sent: {msg!r}")
    print(f" reply: {reply!r}")
    print()

sock.close()
ctx.term()
