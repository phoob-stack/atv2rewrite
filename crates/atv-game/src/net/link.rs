//! How packets get between games. The race only needs [`Link`]: send a
//! datagram to a peer and take the ones that came in. [`Udp`] does that
//! straight between computers (the host's port open to the others);
//! [`Relay`] goes through the online server list (`atv_lobby`), which
//! lists the host's race and passes packets on, so nobody opens a port.

use std::io::ErrorKind;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

use atv_lobby::{Info, Packet};

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
    /// A host's race as it stands, for a server list to show.
    fn advertise(&mut self, _players: u8, _started: bool) {}
    /// Why the link isn't getting through, when something turned it away.
    fn trouble(&self) -> Option<String> {
        None
    }
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
            Ok(a) if self.hosting => format!("Hosting on port {}", a.port()),
            _ => self.peers.first().map_or_else(String::new, |h| format!("Joining {h}")),
        }
    }
}

/// How often a host tells the server list it's still there, and a
/// joining game asks again to join.
const KEEP_UP: Duration = Duration::from_millis(1000);
const ASK_AGAIN: Duration = Duration::from_millis(500);

/// Through the online server list: the server lists a host's race and
/// passes packets between it and the games that joined it.
pub struct Relay {
    socket: UdpSocket,
    server: SocketAddr,
    role: Relayed,
    asked: Option<Instant>,
    trouble: Option<String>,
}

enum Relayed {
    Host { name: String, password: String, info: Info, room: Option<u32> },
    Player { room: u32, password: String, joined: bool },
}

impl Relay {
    /// Lists a race on the server at `server` ("host" or "host:port").
    pub fn host(server: &str, name: &str, password: &str, info: Info) -> std::io::Result<Self> {
        Self::new(server, Relayed::Host { name: name.into(), password: password.into(), info, room: None })
    }

    /// Joins race `room` on the server's list; the host is peer 0.
    pub fn join(server: &str, room: u32, password: &str) -> std::io::Result<(Self, Peer)> {
        Ok((Self::new(server, Relayed::Player { room, password: password.into(), joined: false })?, Peer(0)))
    }

    fn new(server: &str, role: Relayed) -> std::io::Result<Self> {
        let server = atv_lobby::resolve(server)?;
        let socket = UdpSocket::bind(if server.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" })?;
        socket.set_nonblocking(true)?;
        Ok(Self { socket, server, role, asked: None, trouble: None })
    }

    fn tell(&self, p: &Packet) {
        let _ = self.socket.send_to(&p.encode(), self.server);
    }

    /// The host's heartbeat, or asking to join until let in.
    fn keep_up(&mut self) {
        let every = match &self.role {
            Relayed::Host { .. } => KEEP_UP,
            Relayed::Player { joined: true, .. } => return,
            Relayed::Player { .. } => ASK_AGAIN,
        };
        if self.asked.is_some_and(|t| t.elapsed() < every) {
            return;
        }
        self.asked = Some(Instant::now());
        match &self.role {
            Relayed::Host { name, password, info, .. } => self.tell(&Packet::Host { name: name.clone(), password: password.clone(), info: info.clone() }),
            Relayed::Player { room, password, .. } => self.tell(&Packet::Join { room: *room, password: password.clone() }),
        }
    }
}

impl Link for Relay {
    fn send(&mut self, to: Peer, bytes: &[u8]) {
        match self.role {
            Relayed::Host { room: Some(_), .. } | Relayed::Player { joined: true, .. } => self.tell(&Packet::Send { to: to.0, data: bytes.to_vec() }),
            _ => {}
        }
    }

    fn receive(&mut self) -> Option<(Peer, Vec<u8>)> {
        self.keep_up();
        let mut buf = [0u8; MAX_PACKET];
        loop {
            match self.socket.recv_from(&mut buf) {
                Ok((n, from)) if from == self.server => match Packet::decode(&buf[..n]) {
                    Some(Packet::Recv { from, data }) => return Some((Peer(from), data)),
                    Some(Packet::Hosting { room: r }) => {
                        if let Relayed::Host { room, .. } = &mut self.role {
                            *room = Some(r);
                        }
                        self.trouble = None;
                    }
                    Some(Packet::Joined { .. }) => {
                        if let Relayed::Player { joined, .. } = &mut self.role {
                            *joined = true;
                        }
                        self.trouble = None;
                    }
                    Some(Packet::Denied(why)) => self.trouble = Some(why),
                    _ => {}
                },
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::ConnectionReset => continue,
                Err(_) => return None,
            }
        }
    }

    fn describe(&self) -> String {
        match &self.role {
            Relayed::Host { name, room: Some(_), .. } => format!("{name} is on the server list"),
            Relayed::Host { .. } => "Reaching the server list".into(),
            Relayed::Player { joined: true, .. } => "Joined through the server list".into(),
            Relayed::Player { .. } => "Reaching the server list".into(),
        }
    }

    fn advertise(&mut self, players: u8, started: bool) {
        if let Relayed::Host { info, .. } = &mut self.role {
            let news = info.started != started;
            (info.players, info.started) = (players, started);
            // The list hears straight away that the race is on.
            if news {
                self.asked = None;
            }
        }
    }

    fn trouble(&self) -> Option<String> {
        self.trouble.clone()
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        let bye = match self.role {
            Relayed::Host { .. } => Packet::Unhost,
            Relayed::Player { .. } => Packet::Leave,
        };
        for _ in 0..2 {
            self.tell(&bye);
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

    /// Waits for the next datagram.
    fn next(link: &mut dyn Link) -> Option<(Peer, Vec<u8>)> {
        (0..400).find_map(|_| {
            std::thread::sleep(std::time::Duration::from_millis(5));
            link.receive()
        })
    }

    #[test]
    fn games_talk_through_the_server_list() {
        // A server on a thread.
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = socket.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let mut server = atv_lobby::server::Server::new();
            let mut buf = [0u8; MAX_PACKET];
            while let Ok((n, from)) = socket.recv_from(&mut buf) {
                for (to, b) in server.handle(from, &buf[..n], Instant::now()) {
                    let _ = socket.send_to(&b, to);
                }
            }
        });
        let server = format!("127.0.0.1:{port}");
        let mut host = Relay::host(&server, "Phoob's race", "quad", Info { track: "Dunes1".into(), laps: 3, players: 1, max: 8, ..Default::default() }).unwrap();
        for _ in 0..100 {
            host.receive();
            if matches!(host.role, Relayed::Host { room: Some(_), .. }) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let rooms = atv_lobby::list(&server, Duration::from_secs(2)).unwrap();
        assert_eq!(rooms.len(), 1);
        assert!(rooms[0].locked);

        let (mut wrong, _) = Relay::join(&server, rooms[0].id, "nope").unwrap();
        for _ in 0..100 {
            wrong.receive();
            if wrong.trouble().is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(wrong.trouble().as_deref(), Some("WRONG PASSWORD"));

        let (mut player, to_host) = Relay::join(&server, rooms[0].id, "quad").unwrap();
        // Asking to join happens as the game polls; keep sending until in.
        let got = (0..400).find_map(|_| {
            player.send(to_host, b"hello");
            std::thread::sleep(std::time::Duration::from_millis(5));
            player.receive();
            host.receive()
        });
        let (from, bytes) = got.expect("the host heard nothing");
        assert_eq!(bytes, b"hello");
        host.send(from, b"welcome");
        assert_eq!(next(&mut player), Some((to_host, b"welcome".to_vec())));
    }

    #[test]
    fn addresses_take_the_default_port() {
        let (c, _) = Udp::join("127.0.0.1").unwrap();
        assert_eq!(c.peers[0].port(), PORT);
        let (c, _) = Udp::join("127.0.0.1:9000").unwrap();
        assert_eq!(c.peers[0].port(), 9000);
    }
}
