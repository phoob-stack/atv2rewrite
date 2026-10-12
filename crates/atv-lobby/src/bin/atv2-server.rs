//! The online server list and relay (see the `atv_lobby` crate).
//!
//! Usage: atv2-server [--port 7777]
//!
//! Listens on one UDP port; the port can also come from `SERVER_PORT`, as
//! Pterodactyl sets it.

use std::net::UdpSocket;
use std::time::{Duration, Instant};

use atv_lobby::server::Server;
use atv_lobby::{MAX_PACKET, PORT};

fn main() -> std::io::Result<()> {
    let mut port = std::env::var("SERVER_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(PORT);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" => port = args.next().and_then(|p| p.parse().ok()).unwrap_or(port),
            "--help" | "-h" => {
                println!("atv2-server [--port {PORT}]: the ATV 2 online server list and relay, on one UDP port");
                return Ok(());
            }
            other => eprintln!("ignoring unknown argument {other}"),
        }
    }
    let socket = UdpSocket::bind(("0.0.0.0", port))?;
    socket.set_read_timeout(Some(Duration::from_millis(500)))?;
    println!("atv2-server listening on UDP port {port}");
    let mut server = Server::new();
    let mut buf = [0u8; MAX_PACKET];
    let mut last_sweep = Instant::now();
    let mut last_count = (0, 0);
    loop {
        match socket.recv_from(&mut buf) {
            Ok((n, from)) => {
                for (to, bytes) in server.handle(from, &buf[..n], Instant::now()) {
                    let _ = socket.send_to(&bytes, to);
                }
            }
            // Timeouts, and a send that bounced reported on Windows.
            Err(_) => {}
        }
        if last_sweep.elapsed() > Duration::from_secs(1) {
            last_sweep = Instant::now();
            server.sweep(last_sweep);
            let count = (server.rooms(), server.players());
            if count != last_count {
                println!("{} races, {} players joined", count.0, count.1);
                last_count = count;
            }
        }
    }
}
