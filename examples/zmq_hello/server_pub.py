#!/usr/bin/env python3
# examples/zmq_hello/server_pub.py — pyzmq PUB server for the MVL ZMTP SUB client.
#
# Uses the standard pyzmq library (ZMTP 3.x wire protocol).
# Binds on port 5558, publishes 3 weather messages, then exits.
#
# Install: uv pip install pyzmq
#
# Usage:
#   python3 examples/zmq_hello/server_pub.py &
#   cargo run -- run examples/zmq_hello/client_sub.mvl

import time

import zmq

ENDPOINT = "tcp://127.0.0.1:5558"

messages = [
    "weather NYC 72F",
    "weather LA 85F",
    "weather CHI 60F",
]

ctx = zmq.Context()
sock = ctx.socket(zmq.PUB)
sock.bind(ENDPOINT)

# Wait for SUB clients to connect and subscribe (slow joiner problem).
# MVL binaries are pre-built by the Makefile, so 2s is enough.
time.sleep(2)

for msg in messages:
    sock.send_string(msg)
    print(f"  published: {msg!r}")
    time.sleep(0.2)

# Let subscribers receive the last message
time.sleep(1)
sock.close()
ctx.term()
