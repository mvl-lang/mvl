#!/usr/bin/env python3
# examples/zmq_hello/client_push.py — pyzmq PUSH client for the MVL ZMTP server.
#
# Uses the standard pyzmq library (ZMTP 3.x wire protocol).
# Talks to the MVL server_pull.mvl on port 5557.
#
# Install: uv pip install pyzmq
#
# Usage:
#   cargo run -- run examples/zmq_hello/server_pull.mvl &
#   python3 examples/zmq_hello/client_push.py

import time

import zmq

ENDPOINT = "tcp://127.0.0.1:5557"

messages = ["hello from pyzmq PUSH", "the quick brown fox", "PUSH/PULL works!"]

ctx = zmq.Context()
sock = ctx.socket(zmq.PUSH)
sock.connect(ENDPOINT)

# Brief pause for connection establishment
time.sleep(0.3)

for msg in messages:
    sock.send_string(msg)
    print(f"  sent: {msg!r}")

# Let server process the last message before closing
time.sleep(0.5)
sock.close()
ctx.term()
