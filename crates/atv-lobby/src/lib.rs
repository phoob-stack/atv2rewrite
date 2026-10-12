//! The online server list: a small server (`atv2-server`) that hosts
//! announce their races to and players browse, which then relays the
//! race's packets between them, so nobody has to open a port on their
//! router. One UDP port carries it all.
//!
//! A host sends [`Packet::Host`] every second with its race's name,
//! password and [`Info`]; the server lists it as a [`Room`] until it goes
//! quiet. A player asks for the [`Packet::List`], joins a room with its
//! password, and from then on the server passes [`Packet::Send`]s between
//! the player and the host as [`Packet::Recv`]s: the host's peers are
//! numbered from 1, and a joined player only hears from its host (0).
//! What the games say to each other inside them is theirs (the game's
//! `net::wire`).

pub mod server;

use std::io;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

const MAGIC: &[u8; 4] = b"ATVL";
pub const PROTOCOL: u16 = 1;
/// The version of what games say to each other inside the relayed
/// packets (atv-game's `net::wire`): bumped when that changes, so the list
/// can tell races of other builds apart.
pub const GAME_PROTOCOL: u16 = 1;
/// The port the server listens on unless told otherwise.
pub const PORT: u16 = 7777;
/// The biggest datagram either side takes in.
pub const MAX_PACKET: usize = 2048;
/// A list request is at least this long, so the server's answer is never
/// much bigger than the question (a forged sender gains nothing).
pub const LIST_REQUEST: usize = 600;
/// Rooms per list page stop here, in bytes.
const PAGE_BYTES: usize = 1100;
/// The longest race name and password kept.
pub const NAME_LEN: usize = 32;

/// What a host says about its race, for the list.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Info {
    /// The track's folder name ("Dunes1").
    pub track: String,
    pub laps: u8,
    pub players: u8,
    pub max: u8,
    /// The race is on; nobody more can join.
    pub started: bool,
    /// Retail disc, or the preview build: the two don't race together.
    pub retail: bool,
    /// The game's own online protocol version: older games can't join.
    pub game: u16,
}

/// A race on the list.
#[derive(Clone, Debug, PartialEq)]
pub struct Room {
    pub id: u32,
    pub name: String,
    /// It has a password.
    pub locked: bool,
    pub info: Info,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Packet {
    /// A host's race, every second while it's up. An empty password is
    /// none.
    Host { name: String, password: String, info: Info },
    /// The server lists the host's race as this room.
    Hosting { room: u32 },
    /// The host is done.
    Unhost,
    /// A page of the list, please.
    List { page: u16 },
    Rooms { page: u16, pages: u16, rooms: Vec<Room> },
    Join { room: u32, password: String },
    Joined { room: u32 },
    /// Turned away, and why (a wrong password, a full or gone room, a
    /// full server).
    Denied(String),
    /// A game's packet: from a host to peer `to`, from a player to its
    /// host (`to` ignored).
    Send { to: u32, data: Vec<u8> },
    /// A game's packet, passed on: to a host from peer `from`, to a player
    /// from its host (0).
    Recv { from: u32, data: Vec<u8> },
    /// A player is done.
    Leave,
}

impl Packet {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W(Vec::with_capacity(64));
        w.0.extend_from_slice(MAGIC);
        w.u16(PROTOCOL);
        match self {
            Packet::Host { name, password, info } => {
                w.u8(0);
                w.str(name);
                w.str(password);
                w.info(info);
            }
            Packet::Hosting { room } => {
                w.u8(1);
                w.u32(*room);
            }
            Packet::Unhost => w.u8(2),
            Packet::List { page } => {
                w.u8(3);
                w.u16(*page);
                w.0.resize(LIST_REQUEST, 0);
            }
            Packet::Rooms { page, pages, rooms } => {
                w.u8(4);
                w.u16(*page);
                w.u16(*pages);
                w.u8(rooms.len() as u8);
                for r in rooms {
                    w.u32(r.id);
                    w.str(&r.name);
                    w.u8(r.locked as u8);
                    w.info(&r.info);
                }
            }
            Packet::Join { room, password } => {
                w.u8(5);
                w.u32(*room);
                w.str(password);
            }
            Packet::Joined { room } => {
                w.u8(6);
                w.u32(*room);
            }
            Packet::Denied(why) => {
                w.u8(7);
                w.str(why);
            }
            Packet::Send { to, data } => {
                w.u8(8);
                w.u32(*to);
                w.0.extend_from_slice(data);
            }
            Packet::Recv { from, data } => {
                w.u8(9);
                w.u32(*from);
                w.0.extend_from_slice(data);
            }
            Packet::Leave => w.u8(10),
        }
        w.0
    }

    /// None for anything that isn't this protocol's packet.
    pub fn decode(b: &[u8]) -> Option<Self> {
        let mut r = R(b);
        if r.take(4)? != MAGIC || r.u16()? != PROTOCOL {
            return None;
        }
        Some(match r.u8()? {
            0 => Packet::Host { name: r.str()?, password: r.str()?, info: r.info()? },
            1 => Packet::Hosting { room: r.u32()? },
            2 => Packet::Unhost,
            3 if b.len() >= LIST_REQUEST => Packet::List { page: r.u16()? },
            4 => {
                let (page, pages) = (r.u16()?, r.u16()?);
                let n = r.u8()?;
                let rooms = (0..n).map(|_| Some(Room { id: r.u32()?, name: r.str()?, locked: r.u8()? != 0, info: r.info()? })).collect::<Option<_>>()?;
                Packet::Rooms { page, pages, rooms }
            }
            5 => Packet::Join { room: r.u32()?, password: r.str()? },
            6 => Packet::Joined { room: r.u32()? },
            7 => Packet::Denied(r.str()?),
            8 => Packet::Send { to: r.u32()?, data: r.0.to_vec() },
            9 => Packet::Recv { from: r.u32()?, data: r.0.to_vec() },
            10 => Packet::Leave,
            _ => return None,
        })
    }
}

/// Splits rooms into list pages that each fit a datagram.
pub fn pages(rooms: &[Room]) -> Vec<Vec<Room>> {
    let mut out: Vec<Vec<Room>> = vec![Vec::new()];
    let mut size = 0;
    for r in rooms {
        let n = Packet::Rooms { page: 0, pages: 0, rooms: vec![r.clone()] }.encode().len();
        if size + n > PAGE_BYTES || out.last().is_some_and(|p| p.len() == 255) {
            out.push(Vec::new());
            size = 0;
        }
        size += n;
        out.last_mut().unwrap().push(r.clone());
    }
    out
}

/// The server's address from "host" or "host:port".
pub fn resolve(server: &str) -> io::Result<SocketAddr> {
    let server = server.trim();
    let with_port = if server.rsplit_once(':').is_some_and(|(h, p)| !h.ends_with(':') && p.parse::<u16>().is_ok()) { server.to_string() } else { format!("{server}:{PORT}") };
    with_port.to_socket_addrs()?.next().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("no address for {server}")))
}

/// Asks the server at `server` for its whole list, waiting up to
/// `timeout` for it.
pub fn list(server: &str, timeout: Duration) -> io::Result<Vec<Room>> {
    let at = resolve(server)?;
    let socket = UdpSocket::bind(if at.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" })?;
    socket.connect(at)?;
    let until = Instant::now() + timeout;
    let mut got: Vec<Option<Vec<Room>>> = vec![None];
    let mut buf = [0u8; MAX_PACKET];
    let mut asked = Instant::now() - Duration::from_secs(1);
    loop {
        let now = Instant::now();
        if got.iter().all(Option::is_some) {
            break;
        }
        if now >= until {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "the server didn't answer"));
        }
        // Lost answers are asked for again.
        if now - asked > Duration::from_millis(400) {
            for (i, _) in got.iter().enumerate().filter(|(_, p)| p.is_none()) {
                socket.send(&Packet::List { page: i as u16 }.encode())?;
            }
            asked = now;
        }
        socket.set_read_timeout(Some((until - now).min(Duration::from_millis(100)).max(Duration::from_millis(1))))?;
        match socket.recv(&mut buf) {
            Ok(n) => {
                if let Some(Packet::Rooms { page, pages, rooms }) = Packet::decode(&buf[..n]) {
                    got.resize(pages.max(1) as usize, None);
                    if let Some(slot) = got.get_mut(page as usize) {
                        *slot = Some(rooms);
                    }
                }
            }
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::ConnectionReset) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(got.into_iter().flatten().flatten().collect())
}

struct W(Vec<u8>);

impl W {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn str(&mut self, s: &str) {
        let b = &s.as_bytes()[..s.floor_char_boundary(255)];
        self.u8(b.len() as u8);
        self.0.extend_from_slice(b);
    }
    fn info(&mut self, i: &Info) {
        self.str(&i.track);
        self.u8(i.laps);
        self.u8(i.players);
        self.u8(i.max);
        self.u8(i.started as u8 | (i.retail as u8) << 1);
        self.u16(i.game);
    }
}

struct R<'a>(&'a [u8]);

impl R<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        if self.0.len() < n {
            return None;
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Some(a)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn str(&mut self) -> Option<String> {
        let n = self.u8()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
    fn info(&mut self) -> Option<Info> {
        let (track, laps, players, max, flags, game) = (self.str()?, self.u8()?, self.u8()?, self.u8()?, self.u8()?, self.u16()?);
        Some(Info { track, laps, players, max, started: flags & 1 != 0, retail: flags & 2 != 0, game })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room(id: u32) -> Room {
        Room { id, name: format!("Phoob's race {id}"), locked: id % 2 == 0, info: Info { track: "Forest2".into(), laps: 3, players: 2, max: 8, started: false, retail: true, game: 1 } }
    }

    #[test]
    fn packets_round_trip() {
        let all = [
            Packet::Host { name: "Dunes".into(), password: "pw".into(), info: room(1).info },
            Packet::Hosting { room: 7 },
            Packet::Unhost,
            Packet::List { page: 2 },
            Packet::Rooms { page: 0, pages: 1, rooms: vec![room(1), room(2)] },
            Packet::Join { room: 3, password: String::new() },
            Packet::Joined { room: 3 },
            Packet::Denied("WRONG PASSWORD".into()),
            Packet::Send { to: 2, data: vec![1, 2, 3] },
            Packet::Recv { from: 0, data: vec![] },
            Packet::Leave,
        ];
        for p in all {
            assert_eq!(Packet::decode(&p.encode()), Some(p));
        }
        assert_eq!(Packet::decode(b"ATV2\x01\x00\x00"), None);
    }

    #[test]
    fn a_short_list_request_is_ignored() {
        let mut b = Packet::List { page: 0 }.encode();
        b.truncate(16);
        assert_eq!(Packet::decode(&b), None);
    }

    #[test]
    fn list_pages_fit_a_datagram() {
        let rooms: Vec<Room> = (0..100).map(room).collect();
        let p = pages(&rooms);
        assert!(p.len() > 1);
        assert_eq!(p.iter().map(Vec::len).sum::<usize>(), 100);
        for page in &p {
            let n = Packet::Rooms { page: 0, pages: 0, rooms: page.clone() }.encode().len();
            assert!(n <= PAGE_BYTES + 16 && n < LIST_REQUEST * 2 + 16, "{n}");
        }
        assert_eq!(pages(&[]), vec![Vec::<Room>::new()]);
    }
}
