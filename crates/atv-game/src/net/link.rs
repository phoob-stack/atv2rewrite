//! How packets get between games. The race only needs [`Link`]: send a
//! datagram to a peer and take the ones that came in. [`Udp`] does that
//! straight between computers (the host's port open to the others); a
//! relay server that games connect out to can stand in for it later
//! without the race noticing.

use std::io::ErrorKind;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};

/// The port a host listens on unless told otherwise.
pub const PORT: u16 = 7777;
/// The biggest datagram taken in.
const MAX_PACKET: usize = 2048;

/// Another game, as the link names it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Peer(pub u32);

/// Unreliable, unordered datagrams to and from other games. Anything that
/// matters is sent again until it's no longer needed.
pub trait Link: Send + Sync {
    fn send(&mut self, to: Peer, bytes: &[u8]);
    /// The next datagram that came in, if any.
    fn receive(&mut self) -> Option<(Peer, Vec<u8>)>;
    /// Where to find this game, for the lobby to show.
    fn describe(&self) -> String;
}

/// Straight over UDP: the host binds a port, a joining game sends to it.
pub struct Udp {
    socket: UdpSocket,
    /// Each peer's address; a peer is its index.
    peers: Vec<SocketAddr>,
    hosting: bool,
}

impl Udp {
    /// Listens for games joining on `port`.
    pub fn host(port: u16) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(("0.0.0.0", port))?;
        socket.set_nonblocking(true)?;
        Ok(Self { socket, peers: Vec::new(), hosting: true })
    }

    /// Talks to the host at `address` ("192.168.1.20" or "host:port"); the
    /// host is the returned peer.
    pub fn join(address: &str) -> std::io::Result<(Self, Peer)> {
        let address = address.trim();
        let with_port = if address.rsplit_once(':').is_some_and(|(h, p)| !h.ends_with(':') && p.parse::<u16>().is_ok()) { address.to_string() } else { format!("{address}:{PORT}") };
        let host = with_port
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| std::io::Error::new(ErrorKind::NotFound, format!("no address for {address}")))?;
        let any = if host.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" };
        let socket = UdpSocket::bind(any)?;
        socket.set_nonblocking(true)?;
        Ok((Self { socket, peers: vec![host], hosting: false }, Peer(0)))
    }

    fn peer(&mut self, at: SocketAddr) -> Peer {
        let i = self.peers.iter().position(|&p| p == at).unwrap_or_else(|| {
            self.peers.push(at);
            self.peers.len() - 1
        });
        Peer(i as u32)
    }
}

impl Link for Udp {
    fn send(&mut self, to: Peer, bytes: &[u8]) {
        if let Some(at) = self.peers.get(to.0 as usize) {
            // A full send buffer or an unreachable peer just loses the
            // packet, as the network might.
            let _ = self.socket.send_to(bytes, at);
        }
    }

    fn receive(&mut self) -> Option<(Peer, Vec<u8>)> {
        let mut buf = [0u8; MAX_PACKET];
        loop {
            match self.socket.recv_from(&mut buf) {
                Ok((n, from)) => {
                    // A joined game only listens to its host.
                    if !self.hosting && from != self.peers[0] {
                        continue;
                    }
                    return Some((self.peer(from), buf[..n].to_vec()));
                }
                // Windows reports an earlier send that bounced as an error
                // on the next receive; it's not this one's.
                Err(e) if e.kind() == ErrorKind::ConnectionReset => continue,
                Err(_) => return None,
            }
        }
    }

    fn describe(&self) -> String {
        match self.socket.local_addr() {
            Ok(a) if self.hosting => format!("port {}", a.port()),
            _ => self.peers.first().map_or_else(String::new, |h| h.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_joined_game_and_its_host_hear_each_other() {
        let mut host = Udp::host(0).unwrap();
        let port = host.socket.local_addr().unwrap().port();
        let (mut client, to_host) = Udp::join(&format!("127.0.0.1:{port}")).unwrap();
        client.send(to_host, b"hello");
        let got = (0..200).find_map(|_| {
            std::thread::sleep(std::time::Duration::from_millis(5));
            host.receive()
        });
        let (from, bytes) = got.expect("the host heard nothing");
        assert_eq!(bytes, b"hello");
        host.send(from, b"welcome");
        let got = (0..200).find_map(|_| {
            std::thread::sleep(std::time::Duration::from_millis(5));
            client.receive()
        });
        assert_eq!(got, Some((to_host, b"welcome".to_vec())));
    }

    #[test]
    fn addresses_take_the_default_port() {
        let (c, _) = Udp::join("127.0.0.1").unwrap();
        assert_eq!(c.peers[0].port(), PORT);
        let (c, _) = Udp::join("127.0.0.1:9000").unwrap();
        assert_eq!(c.peers[0].port(), 9000);
    }
}
