//! The server's bookkeeping, apart from the socket: rooms and the players
//! in them, what to send for each packet that comes in, and forgetting
//! games that went quiet.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use crate::{Info, NAME_LEN, Packet, Room, pages};

/// A host not heard from for this long has gone, and its room with it.
pub const ROOM_GONE: Duration = Duration::from_secs(15);
/// A player not heard from for this long has gone.
pub const PLAYER_GONE: Duration = Duration::from_secs(30);
const MAX_ROOMS: usize = 256;
const ROOMS_PER_IP: usize = 4;
const MAX_PLAYERS: usize = 2048;
const PLAYERS_PER_ROOM: usize = 16;

struct RoomState {
    host: SocketAddr,
    name: String,
    password: String,
    info: Info,
    seen: Instant,
    next_peer: u32,
}

struct Player {
    room: u32,
    peer: u32,
    seen: Instant,
}

#[derive(Default)]
pub struct Server {
    rooms: HashMap<u32, RoomState>,
    /// Each host's room.
    hosts: HashMap<SocketAddr, u32>,
    players: HashMap<SocketAddr, Player>,
    next_room: u32,
}

/// Cuts a name to [`NAME_LEN`] characters, without control characters.
fn tidy(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(NAME_LEN).collect::<String>().trim().to_string()
}

impl Server {
    pub fn new() -> Self {
        Self { next_room: 1, ..Default::default() }
    }

    pub fn rooms(&self) -> usize {
        self.rooms.len()
    }

    pub fn players(&self) -> usize {
        self.players.len()
    }

    /// What to send, and where, for a packet from `from`.
    pub fn handle(&mut self, from: SocketAddr, bytes: &[u8], now: Instant) -> Vec<(SocketAddr, Vec<u8>)> {
        let Some(packet) = Packet::decode(bytes) else { return Vec::new() };
        let reply = |p: Packet| vec![(from, p.encode())];
        match packet {
            Packet::Host { name, password, info } => {
                if let Some(&id) = self.hosts.get(&from)
                    && let Some(r) = self.rooms.get_mut(&id)
                {
                    (r.name, r.password, r.info, r.seen) = (tidy(&name), tidy(&password), info, now);
                    return reply(Packet::Hosting { room: id });
                }
                let same_ip = self.rooms.values().filter(|r| r.host.ip() == from.ip()).count();
                if self.rooms.len() >= MAX_ROOMS || same_ip >= ROOMS_PER_IP {
                    return reply(Packet::Denied("THE SERVER IS FULL".into()));
                }
                let id = self.next_room;
                self.next_room = self.next_room.wrapping_add(1).max(1);
                self.rooms.insert(id, RoomState { host: from, name: tidy(&name), password: tidy(&password), info, seen: now, next_peer: 1 });
                self.hosts.insert(from, id);
                println!("room {id} up: {:?} from {from}", tidy(&name));
                reply(Packet::Hosting { room: id })
            }
            Packet::Unhost => {
                if let Some(id) = self.hosts.get(&from).copied() {
                    self.close(id);
                }
                Vec::new()
            }
            Packet::List { page } => {
                let mut rooms: Vec<Room> = self
                    .rooms
                    .iter()
                    .map(|(&id, r)| Room { id, name: r.name.clone(), locked: !r.password.is_empty(), info: r.info.clone() })
                    .collect();
                // Open races first, then the newest.
                rooms.sort_by_key(|r| (r.info.started, std::cmp::Reverse(r.id)));
                let all = pages(&rooms);
                let pages = all.len() as u16;
                let rooms = all.into_iter().nth(page as usize).unwrap_or_default();
                reply(Packet::Rooms { page, pages, rooms })
            }
            Packet::Join { room, password } => {
                let Some(r) = self.rooms.get_mut(&room) else { return reply(Packet::Denied("THAT RACE HAS GONE".into())) };
                if let Some(p) = self.players.get_mut(&from).filter(|p| p.room == room) {
                    p.seen = now;
                    return reply(Packet::Joined { room });
                }
                if !r.password.is_empty() && tidy(&password) != r.password {
                    return reply(Packet::Denied("WRONG PASSWORD".into()));
                }
                let in_room = self.players.values().filter(|p| p.room == room).count();
                if in_room >= PLAYERS_PER_ROOM || self.players.len() >= MAX_PLAYERS {
                    return reply(Packet::Denied("THE RACE IS FULL".into()));
                }
                let peer = r.next_peer;
                r.next_peer += 1;
                self.players.insert(from, Player { room, peer, seen: now });
                reply(Packet::Joined { room })
            }
            Packet::Send { to, data } => {
                if let Some(&id) = self.hosts.get(&from) {
                    // Host to one of its players.
                    if let Some(r) = self.rooms.get_mut(&id) {
                        r.seen = now;
                    }
                    let to = self.players.iter().find(|(_, p)| p.room == id && p.peer == to).map(|(a, _)| *a);
                    return to.map(|a| vec![(a, Packet::Recv { from: 0, data }.encode())]).unwrap_or_default();
                }
                // A player to its host.
                let Some(p) = self.players.get_mut(&from) else { return Vec::new() };
                p.seen = now;
                let (room, peer) = (p.room, p.peer);
                self.rooms.get(&room).map(|r| vec![(r.host, Packet::Recv { from: peer, data }.encode())]).unwrap_or_default()
            }
            Packet::Leave => {
                self.players.remove(&from);
                Vec::new()
            }
            Packet::Hosting { .. } | Packet::Rooms { .. } | Packet::Joined { .. } | Packet::Denied(_) | Packet::Recv { .. } => Vec::new(),
        }
    }

    /// Forgets hosts and players that went quiet.
    pub fn sweep(&mut self, now: Instant) {
        let gone: Vec<u32> = self.rooms.iter().filter(|(_, r)| now - r.seen > ROOM_GONE).map(|(&id, _)| id).collect();
        for id in gone {
            self.close(id);
        }
        self.players.retain(|_, p| now - p.seen <= PLAYER_GONE);
    }

    fn close(&mut self, id: u32) {
        if let Some(r) = self.rooms.remove(&id) {
            println!("room {id} down: {:?}", r.name);
            self.hosts.remove(&r.host);
        }
        self.players.retain(|_, p| p.room != id);
    }

    /// Hosts at one IP address (for tests).
    pub fn rooms_at(&self, ip: IpAddr) -> usize {
        self.rooms.values().filter(|r| r.host.ip() == ip).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(n: u8, port: u16) -> SocketAddr {
        SocketAddr::from(([10, 0, 0, n], port))
    }

    fn info() -> Info {
        Info { track: "Dunes1".into(), laps: 3, players: 1, max: 8, started: false, retail: true, game: 1 }
    }

    fn one(out: Vec<(SocketAddr, Vec<u8>)>) -> (SocketAddr, Packet) {
        assert_eq!(out.len(), 1, "{out:?}");
        (out[0].0, Packet::decode(&out[0].1).unwrap())
    }

    #[test]
    fn players_find_a_race_and_talk_through_the_server() {
        let mut s = Server::new();
        let now = Instant::now();
        let (host, a, b) = (at(1, 5000), at(2, 6000), at(3, 6000));
        let (_, p) = one(s.handle(host, &Packet::Host { name: "Phoob's race".into(), password: "quad".into(), info: info() }.encode(), now));
        let Packet::Hosting { room } = p else { panic!("{p:?}") };
        let (_, p) = one(s.handle(a, &Packet::List { page: 0 }.encode(), now));
        let Packet::Rooms { rooms, pages: 1, .. } = p else { panic!("{p:?}") };
        assert_eq!(rooms, vec![Room { id: room, name: "Phoob's race".into(), locked: true, info: info() }]);

        assert_eq!(one(s.handle(a, &Packet::Join { room, password: "nope".into() }.encode(), now)).1, Packet::Denied("WRONG PASSWORD".into()));
        assert_eq!(one(s.handle(a, &Packet::Join { room, password: "quad".into() }.encode(), now)).1, Packet::Joined { room });
        assert_eq!(one(s.handle(b, &Packet::Join { room, password: "quad".into() }.encode(), now)).1, Packet::Joined { room });

        // Players reach the host as peers 1 and 2; the host answers one.
        assert_eq!(one(s.handle(a, &Packet::Send { to: 0, data: vec![7] }.encode(), now)), (host, Packet::Recv { from: 1, data: vec![7] }));
        assert_eq!(one(s.handle(b, &Packet::Send { to: 99, data: vec![8] }.encode(), now)), (host, Packet::Recv { from: 2, data: vec![8] }));
        assert_eq!(one(s.handle(host, &Packet::Send { to: 2, data: vec![9] }.encode(), now)), (b, Packet::Recv { from: 0, data: vec![9] }));
        // Nobody else gets anything through.
        assert!(s.handle(at(4, 1), &Packet::Send { to: 1, data: vec![1] }.encode(), now).is_empty());
        assert!(s.handle(host, &Packet::Send { to: 5, data: vec![1] }.encode(), now).is_empty());
    }

    #[test]
    fn quiet_games_are_forgotten() {
        let mut s = Server::new();
        let now = Instant::now();
        let (host, a) = (at(1, 5000), at(2, 6000));
        s.handle(host, &Packet::Host { name: "x".into(), password: String::new(), info: info() }.encode(), now);
        s.handle(a, &Packet::Join { room: 1, password: "anything".into() }.encode(), now);
        assert_eq!((s.rooms(), s.players()), (1, 1));
        // The host keeps it up with its heartbeat; the player goes quiet.
        let later = now + PLAYER_GONE + Duration::from_secs(1);
        s.handle(host, &Packet::Host { name: "x".into(), password: String::new(), info: info() }.encode(), later - Duration::from_secs(1));
        s.sweep(later);
        assert_eq!((s.rooms(), s.players()), (1, 0));
        s.sweep(later + ROOM_GONE + Duration::from_secs(1));
        assert_eq!(s.rooms(), 0);
    }

    #[test]
    fn one_address_cant_fill_the_list() {
        let mut s = Server::new();
        let now = Instant::now();
        for port in 0..10 {
            s.handle(at(1, port), &Packet::Host { name: "spam".into(), password: String::new(), info: info() }.encode(), now);
        }
        assert_eq!(s.rooms_at(at(1, 0).ip()), ROOMS_PER_IP);
    }
}
