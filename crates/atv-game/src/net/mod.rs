//! Online races, a first cut: one game hosts (`--host`), others join it by
//! address (`--join`). The host runs the computer riders and tells
//! everyone where every quad is 30 times a second; each player's game runs
//! its own quad, so steering answers at once, and tells the host where it
//! is. Everyone else's quad is a puppet (see [`crate::racers`]), shown a
//! tenth of a second behind its latest news so there's always a state
//! either side of the drawn moment.
//!
//! Single Race only. The lobby is the loading screen: the host starts the
//! race when the players are in. Quads don't bump or kick across games
//! yet.
//!
//! Games reach each other straight (the host's port open) or through the
//! online server list, which lists the host's race with its name, password
//! and track and passes the packets on (see [`link::Relay`]).

pub mod link;
pub mod wire;

use std::collections::{HashMap, VecDeque};

use bevy::prelude::*;

use crate::frontend::Roster;
use crate::opponents::Opponents;
use crate::race::{RaceProgress, Report};
use crate::racers::{AllRacers, Control, RacerId, RacerState};
use crate::{AppState, Mode, Selection};
use link::{Link, Peer};
use wire::{Msg, Seat};

/// How far behind its latest news another quad is shown.
const DELAY: f64 = 0.1;
/// How often quads' states go out.
const SEND_EVERY: f64 = 1.0 / 30.0;
/// How often the lobby and other news that has to get through go out.
const NEWS_EVERY: f64 = 0.25;
/// The host lets the countdown go without a game that hasn't loaded the
/// race after this long.
const READY_WAIT: f64 = 20.0;
/// A game not heard from for this long has gone.
const GONE: f64 = 10.0;
/// The most quads on a grid.
const GRID: usize = 8;

/// What makes a race online, kept with the selection.
#[derive(Clone, Debug, Default)]
pub struct NetRace {
    pub host: bool,
    /// This game's own racer id (0 on the host).
    pub you: RacerId,
    /// On the host, the players who joined; on a joined game, the host's
    /// whole line-up.
    pub seats: Vec<Seat>,
}

/// The online game, while there is one.
#[derive(Resource)]
pub struct Online {
    link: Box<dyn Link>,
    role: Role,
    /// Seconds since going online.
    clock: f64,
    next_send: f64,
    next_news: f64,
    /// The countdown has been let go.
    go: bool,
    /// Every other racer's latest states, by id.
    heard: HashMap<RacerId, Heard>,
    /// For the lobby: what's going on.
    pub status: String,
    /// The disc this game reads, which a joining game must match.
    retail: bool,
}

enum Role {
    Host {
        joined: Vec<Joined>,
        /// When the race was started.
        started: Option<f64>,
    },
    Client {
        host: Peer,
        /// The riders in the host's lobby.
        players: Vec<String>,
        refused: bool,
        heard: f64,
    },
}

/// A game that joined the host.
struct Joined {
    peer: Peer,
    rider: String,
    quad: String,
    id: Option<RacerId>,
    ready: bool,
    heard: f64,
}

/// Shown on the loading screen until the race is on.
#[derive(Resource)]
pub struct Lobby;

/// What another game said about one racer lately, with that game's clock.
#[derive(Default)]
struct Heard {
    got: VecDeque<(f64, RacerState, Report)>,
    /// Their clock less ours, as near as the packets tell.
    offset: Option<f64>,
}

impl Heard {
    fn push(&mut self, theirs: f64, ours: f64, state: RacerState, report: Report) {
        // The quickest packet sets the offset; it creeps back so a clock
        // running slow is followed.
        let seen = theirs - ours;
        self.offset = Some(self.offset.map_or(seen, |o| (o - 0.0005).max(seen)));
        if self.got.back().is_some_and(|(t, ..)| *t >= theirs) {
            return;
        }
        self.got.push_back((theirs, state, report));
        while self.got.len() > 2 && self.got[1].0 < theirs - 1.0 {
            self.got.pop_front();
        }
    }

    /// The racer as it was `DELAY` before the latest news, between the two
    /// states either side.
    fn at(&self, ours: f64) -> Option<(RacerState, &Report)> {
        let when = ours + self.offset? - DELAY;
        let after = self.got.iter().position(|(t, ..)| *t >= when);
        match after {
            Some(0) | None => {
                let (_, s, r) = if after.is_none() { self.got.back()? } else { self.got.front()? };
                Some((s.clone(), r))
            }
            Some(i) => {
                let ((t0, a, _), (t1, b, r)) = (&self.got[i - 1], &self.got[i]);
                let t = ((when - t0) / (t1 - t0).max(1e-6)) as f32;
                Some((RacerState::lerp(a, b, t.clamp(0.0, 1.0)), r))
            }
        }
    }
}

impl Online {
    /// Hosts on `port`.
    pub fn host(port: u16, retail: bool) -> std::io::Result<Self> {
        Ok(Self::hosting(Box::new(link::Udp::host(port)?), retail))
    }

    /// Hosts on the server list at `server`, as `name` (and with
    /// `password`, if not empty).
    pub fn host_listed(server: &str, name: &str, password: &str, retail: bool, sel: &Selection) -> std::io::Result<Self> {
        let info = atv_lobby::Info { track: sel.track.clone(), laps: sel.laps as u8, players: 1, max: GRID as u8, started: false, retail, game: wire::PROTOCOL };
        Ok(Self::hosting(Box::new(link::Relay::host(server, name, password, info)?), retail))
    }

    /// Joins the host at `address`.
    pub fn join(address: &str, retail: bool) -> std::io::Result<Self> {
        let (link, host) = link::Udp::join(address)?;
        Ok(Self::joining(Box::new(link), host, retail))
    }

    /// Joins race `room` on the server list at `server`.
    pub fn join_listed(server: &str, room: u32, password: &str, retail: bool) -> std::io::Result<Self> {
        let (link, host) = link::Relay::join(server, room, password)?;
        Ok(Self::joining(Box::new(link), host, retail))
    }

    fn hosting(link: Box<dyn Link>, retail: bool) -> Self {
        Self::new(link, Role::Host { joined: Vec::new(), started: None }, retail)
    }

    fn joining(link: Box<dyn Link>, host: Peer, retail: bool) -> Self {
        Self::new(link, Role::Client { host, players: Vec::new(), refused: false, heard: 0.0 }, retail)
    }

    fn new(link: Box<dyn Link>, role: Role, retail: bool) -> Self {
        let status = link.describe().to_uppercase();
        Self { link, role, clock: 0.0, next_send: 0.0, next_news: 0.0, go: false, heard: HashMap::new(), status, retail }
    }

    fn send(&mut self, to: Peer, m: &Msg) {
        self.link.send(to, &m.encode());
    }

    /// The lobby's lines: who's in, and what happens next.
    pub fn lobby_lines(&self, sel: &Selection, roster: &Roster) -> Vec<String> {
        let name = |model: &str| roster.riders.iter().find(|r| r.model.eq_ignore_ascii_case(model)).map_or(model.to_uppercase(), |r| r.name.to_uppercase());
        // What's going on: something turned us away, or for a host how
        // others find it.
        let headline = match (self.link.trouble(), &self.role) {
            (Some(why), _) => why.to_uppercase(),
            (None, Role::Host { .. }) => self.link.describe().to_uppercase(),
            (None, Role::Client { .. }) => self.status.clone(),
        };
        let mut lines = vec![headline];
        match &self.role {
            Role::Host { joined, .. } => {
                lines.push(format!("{} - {} LAPS", sel.track.to_uppercase(), sel.laps));
                lines.push(format!("1. {} (YOU)", name(&sel.rider)));
                for (i, j) in joined.iter().enumerate() {
                    lines.push(format!("{}. {}", i + 2, name(&j.rider)));
                }
                lines.push("PRESS ENTER OR A TO START".into());
                lines.push("ESCAPE OR B TO LEAVE".into());
            }
            Role::Client { players, refused, .. } => {
                for (i, p) in players.iter().enumerate() {
                    lines.push(format!("{}. {}", i + 1, p.to_uppercase()));
                }
                if !refused && self.link.trouble().is_none() {
                    lines.push(if players.is_empty() { "LOOKING FOR THE HOST".into() } else { "WAITING FOR THE HOST TO START".into() });
                }
                lines.push("ESCAPE OR B TO LEAVE".into());
            }
        }
        lines
    }
}

pub struct NetPlugin;

impl Plugin for NetPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, (pump, news).chain().run_if(resource_exists::<Online>))
            .add_systems(Update, (start_hosting, leave_lobby).run_if(resource_exists::<Lobby>).run_if(in_state(AppState::Loading)))
            .add_systems(
                FixedUpdate,
                (hold, tell_puppets).chain().after(crate::drive).before(crate::opponents::drive_opponents).run_if(resource_exists::<Online>).run_if(in_state(AppState::Race)),
            )
            .add_systems(FixedUpdate, send_states.after(crate::opponents::drive_opponents).run_if(resource_exists::<Online>).run_if(in_state(AppState::Race)))
            .add_systems(OnExit(AppState::Race), leave);
    }
}

/// Takes in what came over the network.
#[allow(clippy::too_many_arguments)]
fn pump(
    mut commands: Commands,
    time: Res<Time<Real>>,
    mut online: ResMut<Online>,
    mut sel: ResMut<Selection>,
    lobby: Option<Res<Lobby>>,
    screens: Query<Entity, With<crate::loading::LoadingScreen>>,
) {
    let on = &mut *online;
    on.clock = time.elapsed_secs_f64();
    let now = on.clock;
    let mut replies = Vec::new();
    while let Some((from, bytes)) = on.link.receive() {
        let Some(msg) = Msg::decode(&bytes) else { continue };
        match &mut on.role {
            Role::Host { joined, started } => {
                let known = joined.iter().position(|j| j.peer == from);
                if let Some(j) = known.map(|i| &mut joined[i]) {
                    j.heard = now;
                }
                match msg {
                    Msg::Hello { retail, rider, quad } => {
                        let why = if retail != on.retail {
                            Some("THE HOST HAS THE OTHER VERSION OF THE GAME")
                        } else if known.is_none() && started.is_some() {
                            Some("THE RACE HAS ALREADY STARTED")
                        } else if known.is_none() && joined.len() + 1 >= GRID {
                            Some("THE RACE IS FULL")
                        } else {
                            None
                        };
                        match (why, known) {
                            (Some(why), _) => replies.push((from, Msg::Refused(why.into()))),
                            (None, Some(i)) if started.is_none() => (joined[i].rider, joined[i].quad) = (rider, quad),
                            (None, Some(_)) => {}
                            (None, None) => {
                                info!("online: {rider} on {quad} joined");
                                joined.push(Joined { peer: from, rider, quad, id: None, ready: false, heard: now });
                            }
                        }
                    }
                    Msg::Ready => {
                        if let Some(i) = known {
                            joined[i].ready = true;
                        }
                    }
                    Msg::Mine { time, state, report } => {
                        if let Some(id) = known.and_then(|i| joined[i].id) {
                            on.heard.entry(id).or_default().push(time as f64, now, state, report);
                        }
                    }
                    Msg::Bye => {
                        if let Some(i) = known.filter(|_| started.is_none()) {
                            joined.remove(i);
                        }
                    }
                    _ => {}
                }
            }
            Role::Client { host, players, refused, heard } => {
                if from != *host {
                    continue;
                }
                *heard = now;
                match msg {
                    Msg::Refused(why) => {
                        on.status = why;
                        *refused = true;
                    }
                    Msg::Lobby { track, laps, players: p } => {
                        *players = p;
                        on.status = format!("{} - {laps} LAPS", track.to_uppercase());
                    }
                    Msg::Race { you, track, laps, seats } if sel.net.is_none() && lobby.is_some() => {
                        info!("online: racing {track} as racer {}", you.0);
                        sel.track = track;
                        sel.laps = (laps as u32).clamp(1, 9);
                        sel.mode = Mode::SingleRace;
                        sel.players = 1;
                        if let Some(me) = seats.iter().find(|s| s.id == you) {
                            (sel.rider, sel.quad) = (me.rider.clone(), me.quad.clone());
                        }
                        sel.net = Some(NetRace { host: false, you, seats });
                        commands.remove_resource::<Lobby>();
                        // The loading screen again, for the host's track.
                        for e in &screens {
                            commands.entity(e).despawn();
                        }
                    }
                    Msg::Snapshot { time, go, racers } => {
                        on.go |= go;
                        let you = sel.net.as_ref().map(|n| n.you);
                        for (id, state, report) in racers {
                            if Some(id) != you {
                                on.heard.entry(id).or_default().push(time as f64, now, state, report);
                            }
                        }
                    }
                    Msg::Bye => on.status = "THE HOST HAS LEFT".into(),
                    _ => {}
                }
            }
        }
    }
    for (to, m) in replies {
        on.send(to, &m);
    }
}

/// What has to get through, sent again a few times a second until it
/// doesn't matter: the lobby, the race's line-up, being ready, joining.
fn news(mut online: ResMut<Online>, sel: Res<Selection>, roster: Res<Roster>, all: AllRacers, opponents: Option<Res<Opponents>>, state: Res<State<AppState>>) {
    let on = &mut *online;
    if on.clock < on.next_news {
        return;
    }
    on.next_news = on.clock + NEWS_EVERY;
    let racing = *state.get() == AppState::Race;
    let retail = on.retail;
    let now = on.clock;
    let mut out = Vec::new();
    match &mut on.role {
        Role::Host { joined, started } => {
            on.link.advertise(1 + joined.len() as u8, started.is_some());
            let name = |model: &str| roster.riders.iter().find(|r| r.model.eq_ignore_ascii_case(model)).map_or(model.to_string(), |r| r.name.clone());
            if started.is_none() {
                // A game that went quiet has gone.
                joined.retain(|j| now - j.heard < GONE);
                let players: Vec<String> = std::iter::once(name(&sel.rider)).chain(joined.iter().map(|j| name(&j.rider))).collect();
                for j in joined.iter() {
                    out.push((j.peer, Msg::Lobby { track: sel.track.clone(), laps: sel.laps as u8, players: players.clone() }));
                }
            } else if racing && let Some(opp) = opponents.as_deref() {
                // The line-up, to anyone yet to load it.
                let mut seats = vec![Seat { id: all.player_id(), rider: sel.rider.clone(), quad: sel.quad.clone(), human: true }];
                seats.extend(opp.racers.iter().map(|r| Seat { id: r.id, rider: r.model.clone(), quad: r.quad.clone(), human: r.remote || r.human().is_some() }));
                for j in joined.iter().filter(|j| !j.ready) {
                    if let Some(you) = j.id {
                        out.push((j.peer, Msg::Race { you, track: sel.track.clone(), laps: sel.laps as u8, seats: seats.clone() }));
                    }
                }
            }
        }
        Role::Client { host, refused, .. } => {
            if sel.net.is_none() && !*refused {
                out.push((*host, Msg::Hello { retail, rider: sel.rider.clone(), quad: sel.quad.clone() }));
            } else if racing && !on.go {
                out.push((*host, Msg::Ready));
            }
        }
    }
    for (to, m) in out {
        on.send(to, &m);
    }
}

/// The host starts the race from the lobby: the players in it get their
/// seats on the grid behind the host's.
fn start_hosting(mut commands: Commands, keys: Res<ButtonInput<KeyCode>>, pads: Query<&Gamepad>, mut online: ResMut<Online>, mut sel: ResMut<Selection>) {
    let on = &mut *online;
    let Role::Host { joined, started } = &mut on.role else { return };
    let pressed = keys.just_pressed(KeyCode::Enter) || pads.iter().any(|p| p.just_pressed(GamepadButton::South) || p.just_pressed(GamepadButton::Start));
    if !pressed {
        return;
    }
    let seats = joined
        .iter_mut()
        .enumerate()
        .map(|(i, j)| {
            let id = RacerId(i as u8 + 1);
            j.id = Some(id);
            Seat { id, rider: j.rider.clone(), quad: j.quad.clone(), human: true }
        })
        .collect();
    info!("online: starting with {} joined", joined.len());
    *started = Some(on.clock);
    sel.mode = Mode::SingleRace;
    sel.players = 1;
    sel.net = Some(NetRace { host: true, you: RacerId::PLAYER_1, seats });
    commands.remove_resource::<Lobby>();
}

/// Escape (or B) in the lobby goes back to the menus, offline.
fn leave_lobby(mut commands: Commands, keys: Res<ButtonInput<KeyCode>>, pads: Query<&Gamepad>, mut online: ResMut<Online>, mut next: ResMut<NextState<AppState>>) {
    let pressed = keys.just_pressed(KeyCode::Escape) || pads.iter().any(|p| p.just_pressed(GamepadButton::East) || p.just_pressed(GamepadButton::Select));
    if !pressed {
        return;
    }
    let peers: Vec<Peer> = match &online.role {
        Role::Host { joined, .. } => joined.iter().map(|j| j.peer).collect(),
        Role::Client { host, .. } => vec![*host],
    };
    for p in peers {
        online.send(p, &Msg::Bye);
    }
    commands.remove_resource::<Online>();
    commands.remove_resource::<Lobby>();
    next.set(AppState::FrontEnd);
}

/// Keeps everyone on the grid until the host lets the countdown go: once
/// every joined game has loaded the race, or after [`READY_WAIT`].
fn hold(mut online: ResMut<Online>, mut progress: Option<ResMut<RaceProgress>>, mut opponents: Option<ResMut<Opponents>>) {
    let on = &mut *online;
    if let Role::Host { joined, started } = &on.role {
        let waited = started.is_some_and(|t| on.clock - t > READY_WAIT);
        on.go |= opponents.is_some() && (waited || joined.iter().all(|j| j.ready || j.id.is_none()));
    }
    if on.go {
        return;
    }
    if let Some(p) = progress.as_mut() {
        p.countdown = RaceProgress::COUNTDOWN;
    }
    for r in opponents.iter_mut().flat_map(|o| o.racers.iter_mut()) {
        r.progress.countdown = RaceProgress::COUNTDOWN;
    }
}

/// Shows each puppet where the game driving it last said, a little behind.
fn tell_puppets(time: Res<Time>, online: Res<Online>, opponents: Option<ResMut<Opponents>>) {
    let Some(mut opp) = opponents else { return };
    let span = time.delta_secs();
    for r in opp.racers.iter_mut().filter(|r| r.control == Control::Puppet) {
        if let Some((state, report)) = online.heard.get(&r.id).and_then(|h| h.at(online.clock)) {
            r.tell(&state, span);
            r.progress.take_report(report);
        }
    }
}

/// Sends quads' states: the host every racer's, a joined game its own.
fn send_states(mut online: ResMut<Online>, sel: Res<Selection>, all: AllRacers) {
    let on = &mut *online;
    if on.clock < on.next_send {
        return;
    }
    on.next_send = on.clock + SEND_EVERY;
    let time = on.clock as f32;
    let mut out = Vec::new();
    match &on.role {
        Role::Host { joined, .. } => {
            let racers: Vec<_> = all.ids().into_iter().filter_map(|id| Some((id, all.state(id)?, all.report(id)?))).collect();
            let snapshot = Msg::Snapshot { time, go: on.go, racers };
            for j in joined.iter().filter(|j| j.ready && on.clock - j.heard < GONE) {
                out.push((j.peer, snapshot.clone()));
            }
        }
        Role::Client { host, .. } => {
            let me = all.player_id();
            if sel.net.is_some()
                && let (Some(state), Some(report)) = (all.state(me), all.report(me))
            {
                out.push((*host, Msg::Mine { time, state, report }));
            }
        }
    }
    for (to, m) in out {
        on.send(to, &m);
    }
}

/// Leaving the race leaves the online game.
fn leave(mut commands: Commands, online: Option<ResMut<Online>>, mut sel: ResMut<Selection>) {
    sel.net = None;
    let Some(mut on) = online else { return };
    let peers: Vec<Peer> = match &on.role {
        Role::Host { joined, .. } => joined.iter().map(|j| j.peer).collect(),
        Role::Client { host, .. } => vec![*host],
    };
    for p in peers {
        for _ in 0..3 {
            on.send(p, &Msg::Bye);
        }
    }
    commands.remove_resource::<Online>();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: f32) -> RacerState {
        let mut s = RacerState::default();
        s.quad.position.x = x;
        s
    }

    #[test]
    fn others_are_shown_a_little_behind_their_news() {
        let mut h = Heard::default();
        // Their clock runs 100 s ahead of ours; packets take 20 ms.
        for i in 0..10 {
            let theirs = 100.0 + i as f64 * SEND_EVERY;
            h.push(theirs, theirs - 100.0 + 0.02, at(i as f32), Report::default());
        }
        let ours = 9.0 * SEND_EVERY + 0.02;
        let (s, _) = h.at(ours).unwrap();
        // DELAY behind the last state: 3 packets back.
        assert!((s.quad.position.x - (9.0 - (DELAY / SEND_EVERY) as f32)).abs() < 0.01, "{}", s.quad.position.x);
        // Halfway between two packets, halfway between their states.
        let (s, _) = h.at(ours + SEND_EVERY / 2.0).unwrap();
        assert!((s.quad.position.x - 6.5).abs() < 0.01, "{}", s.quad.position.x);
        // Late news is dropped; with nothing newer, the last state stays.
        h.push(100.0, 0.5, at(-1.0), Report::default());
        let (s, _) = h.at(100.0).unwrap();
        assert_eq!(s.quad.position.x, 9.0);
    }
}
