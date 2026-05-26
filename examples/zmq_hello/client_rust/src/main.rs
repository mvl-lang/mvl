// examples/zmq_hello/client_rust — Rust ZMQ REQ client for the MVL ZMTP server.
//
// Uses the `zeromq` crate (pure-Rust ZMTP 3.x implementation).
// Talks to the MVL server_zmtp.mvl on port 5556.
//
// Build & run:
//   cd examples/zmq_hello/client_rust && cargo run
//
// Or via the Makefile:
//   make -C examples/zmq_hello test-rust

use zeromq::{Socket, SocketRecv, SocketSend};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let endpoint = "tcp://127.0.0.1:5556";

    let messages = [
        "hello from Rust",
        "the quick brown fox",
        "MVL + Rust ZMQ works",
    ];

    let mut socket = zeromq::ReqSocket::new();
    socket.connect(endpoint).await?;

    for msg in &messages {
        socket.send((*msg).into()).await?;
        let reply = socket.recv().await?;
        let reply_str = String::from_utf8_lossy(reply.get(0).map(|f| f.as_ref()).unwrap_or(b""));
        println!("  sent: {:?}", msg);
        println!(" reply: {:?}", reply_str.as_ref());
        println!();
    }

    Ok(())
}
