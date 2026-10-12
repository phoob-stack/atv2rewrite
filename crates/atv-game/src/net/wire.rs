//! What games send each other in an online race, and how it's packed into
//! a datagram. Everything is sent again until it no longer matters (the
//! lobby a few times a second, each quad's state 30 times a second), so
//! no message needs to arrive.

use atv_sim::QuadPose;
use atv_sim::vehicle::WheelState;
use bevy::math::{Quat, Vec3};

use crate::boost::Boost;
use crate::race::Report;
use crate::racers::{RacerId, RacerState};

/// Every packet starts with this and the protocol version: games of
/// different builds don't talk.
const MAGIC: &[u8; 4] = b"ATV2";
pub const PROTOCOL: u16 = atv_lobby::GAME_PROTOCOL;

/// One quad in an online race's line-up.
#[derive(Clone, Debug, PartialEq)]
pub struct Seat {
    pub id: RacerId,
    /// The rider's model folder and the quad.
    pub rider: String,
    pub quad: String,
    /// A player (on the host or a joined game), not a computer rider.
    pub human: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Msg {
    /// A game asking to join, with its disc (retail or the preview build)
    /// and its player's picks.
    Hello { retail: bool, rider: String, quad: String },
    /// The host turned the game away.
    Refused(String),
    /// The host's lobby: the track and the players' riders so far, and
    /// the seat the receiver will have.
    Lobby { track: String, laps: u8, players: Vec<String> },
    /// The race is on: load it with this line-up; `you` is the receiver's
    /// seat.
    Race { you: RacerId, track: String, laps: u8, seats: Vec<Seat> },
    /// A joined game has loaded the race.
    Ready,
    /// The host's race: its clock, whether the countdown has been let go,
    /// and every racer.
    Snapshot { time: f32, go: bool, racers: Vec<(RacerId, RacerState, Report)> },
    /// A joined game's own quad, with its clock.
    Mine { time: f32, state: RacerState, report: Report },
    Bye,
}

impl Msg {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W(Vec::with_capacity(256));
        w.0.extend_from_slice(MAGIC);
        w.u16(PROTOCOL);
        match self {
            Msg::Hello { retail, rider, quad } => {
                w.u8(0);
                w.bool(*retail);
                w.str(rider);
                w.str(quad);
            }
            Msg::Refused(why) => {
                w.u8(1);
                w.str(why);
            }
            Msg::Lobby { track, laps, players } => {
                w.u8(2);
                w.str(track);
                w.u8(*laps);
                w.u8(players.len() as u8);
                for p in players {
                    w.str(p);
                }
            }
            Msg::Race { you, track, laps, seats } => {
                w.u8(3);
                w.u8(you.0);
                w.str(track);
                w.u8(*laps);
                w.u8(seats.len() as u8);
                for s in seats {
                    w.u8(s.id.0);
                    w.str(&s.rider);
                    w.str(&s.quad);
                    w.bool(s.human);
                }
            }
            Msg::Ready => w.u8(4),
            Msg::Snapshot { time, go, racers } => {
                w.u8(5);
                w.f32(*time);
                w.bool(*go);
                w.u8(racers.len() as u8);
                for (id, s, r) in racers {
                    w.u8(id.0);
                    w.state(s);
                    w.report(r);
                }
            }
            Msg::Mine { time, state, report } => {
                w.u8(6);
                w.f32(*time);
                w.state(state);
                w.report(report);
            }
            Msg::Bye => w.u8(7),
        }
        w.0
    }

    /// None for anything that isn't this build's message.
    pub fn decode(b: &[u8]) -> Option<Self> {
        let mut r = R(b);
        if r.take(4)? != MAGIC || r.u16()? != PROTOCOL {
            return None;
        }
        let msg = match r.u8()? {
            0 => Msg::Hello { retail: r.bool()?, rider: r.str()?, quad: r.str()? },
            1 => Msg::Refused(r.str()?),
            2 => {
                let (track, laps) = (r.str()?, r.u8()?);
                let n = r.u8()?;
                Msg::Lobby { track, laps, players: (0..n).map(|_| r.str()).collect::<Option<_>>()? }
            }
            3 => {
                let (you, track, laps) = (RacerId(r.u8()?), r.str()?, r.u8()?);
                let n = r.u8()?;
                let seats = (0..n).map(|_| Some(Seat { id: RacerId(r.u8()?), rider: r.str()?, quad: r.str()?, human: r.bool()? })).collect::<Option<_>>()?;
                Msg::Race { you, track, laps, seats }
            }
            4 => Msg::Ready,
            5 => {
                let (time, go) = (r.f32()?, r.bool()?);
                let n = r.u8()?;
                let racers = (0..n).map(|_| Some((RacerId(r.u8()?), r.state()?, r.report()?))).collect::<Option<_>>()?;
                Msg::Snapshot { time, go, racers }
            }
            6 => Msg::Mine { time: r.f32()?, state: r.state()?, report: r.report()? },
            7 => Msg::Bye,
            _ => return None,
        };
        r.0.is_empty().then_some(msg)
    }
}

struct W(Vec<u8>);

impl W {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn bool(&mut self, v: bool) {
        self.u8(v as u8);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i16(&mut self, v: f32, scale: f32) {
        self.0.extend_from_slice(&((v * scale).round().clamp(-32767.0, 32767.0) as i16).to_le_bytes());
    }
    /// -1..=1 in a byte.
    fn i8(&mut self, v: f32) {
        self.u8((v.clamp(-1.0, 1.0) * 127.0).round() as i8 as u8);
    }
    /// 0..=1 in a byte.
    fn unit(&mut self, v: f32) {
        self.u8((v.clamp(0.0, 1.0) * 255.0).round() as u8);
    }
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn vec3(&mut self, v: Vec3) {
        for c in v.to_array() {
            self.f32(c);
        }
    }
    fn str(&mut self, s: &str) {
        let b = &s.as_bytes()[..s.len().min(255)];
        self.u8(b.len() as u8);
        self.0.extend_from_slice(b);
    }
    /// A time in seconds, to the millisecond, or none.
    fn time(&mut self, t: Option<f32>) {
        self.f32(t.unwrap_or(-1.0));
    }
    fn timer(&mut self, t: Option<(f32, bool)>) {
        match t {
            Some((t, right)) => {
                self.u8(1 + right as u8);
                self.i16(t, 1000.0);
            }
            None => self.u8(0),
        }
    }

    fn state(&mut self, s: &RacerState) {
        let q = &s.quad;
        self.vec3(q.position);
        for c in q.rotation.normalize().to_array() {
            self.i16(c, 32767.0);
        }
        self.vec3(q.velocity);
        for c in q.angular_velocity.to_array() {
            self.i16(c, 100.0);
        }
        self.u8(q.wheels.len() as u8);
        for w in &q.wheels {
            self.i16(w.compression, 10_000.0);
            self.u16((w.spin.rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU * 65535.0) as u16);
            self.i16(w.steer, 10_000.0);
            self.bool(w.contact);
            self.u8(w.surface);
            for c in w.contact_normal.to_array() {
                self.i8(c);
            }
        }
        self.i16(q.steer, 32767.0);
        self.unit(q.throttle);
        self.unit(q.brake);
        self.i16(q.lean, 32767.0);
        self.u8(q.gear as u8);
        self.f32(q.revs);
        self.unit(q.preload);
        self.f32(q.air_time);
        self.i16(s.lean, 32767.0);
        self.f32(s.boost.meter);
        self.bool(s.boost.on);
        self.timer(s.kick);
        self.timer(s.flinch);
        match s.trick {
            Some((def, t)) => {
                self.u8(1);
                self.u16(def as u16);
                self.f32(t);
            }
            None => self.u8(0),
        }
    }

    fn report(&mut self, r: &Report) {
        self.f32(r.clock);
        self.u8(r.lap_times.len() as u8);
        // To the hundredth, as the times are shown.
        for &t in &r.lap_times {
            self.u16((t * 100.0).round().clamp(0.0, 65535.0) as u16);
        }
        self.time(r.finished);
        self.bool(r.armed);
        self.unit(r.lap_fraction);
        self.bool(r.dnf);
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
    fn bool(&mut self) -> Option<bool> {
        Some(self.u8()? != 0)
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    fn i16(&mut self, scale: f32) -> Option<f32> {
        Some(i16::from_le_bytes(self.take(2)?.try_into().ok()?) as f32 / scale)
    }
    fn i8(&mut self) -> Option<f32> {
        Some(self.u8()? as i8 as f32 / 127.0)
    }
    fn unit(&mut self) -> Option<f32> {
        Some(self.u8()? as f32 / 255.0)
    }
    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?)).filter(|v| v.is_finite())
    }
    fn vec3(&mut self) -> Option<Vec3> {
        Some(Vec3::new(self.f32()?, self.f32()?, self.f32()?))
    }
    fn str(&mut self) -> Option<String> {
        let n = self.u8()? as usize;
        Some(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
    fn time(&mut self) -> Option<Option<f32>> {
        let t = self.f32()?;
        Some((t >= 0.0).then_some(t))
    }
    fn timer(&mut self) -> Option<Option<(f32, bool)>> {
        Some(match self.u8()? {
            0 => None,
            k => Some((self.i16(1000.0)?, k == 2)),
        })
    }

    fn state(&mut self) -> Option<RacerState> {
        let position = self.vec3()?;
        let rotation = Quat::from_array([self.i16(32767.0)?, self.i16(32767.0)?, self.i16(32767.0)?, self.i16(32767.0)?]).normalize();
        let velocity = self.vec3()?;
        let angular_velocity = Vec3::new(self.i16(100.0)?, self.i16(100.0)?, self.i16(100.0)?);
        let n = self.u8()?;
        let wheels = (0..n)
            .map(|_| {
                Some(WheelState {
                    compression: self.i16(10_000.0)?,
                    spin: self.u16()? as f32 / 65535.0 * std::f32::consts::TAU,
                    steer: self.i16(10_000.0)?,
                    contact: self.bool()?,
                    surface: self.u8()?,
                    contact_normal: Vec3::new(self.i8()?, self.i8()?, self.i8()?),
                })
            })
            .collect::<Option<_>>()?;
        let quad = QuadPose {
            position,
            rotation,
            velocity,
            angular_velocity,
            wheels,
            steer: self.i16(32767.0)?,
            throttle: self.unit()?,
            brake: self.unit()?,
            lean: self.i16(32767.0)?,
            gear: self.u8()? as usize,
            revs: self.f32()?,
            preload: self.unit()?,
            air_time: self.f32()?,
        };
        Some(RacerState {
            quad,
            lean: self.i16(32767.0)?,
            boost: Boost { meter: self.f32()?, on: self.bool()? },
            kick: self.timer()?,
            flinch: self.timer()?,
            trick: match self.u8()? {
                0 => None,
                _ => Some((self.u16()? as usize, self.f32()?)),
            },
        })
    }

    fn report(&mut self) -> Option<Report> {
        let clock = self.f32()?;
        let n = self.u8()?;
        Some(Report {
            clock,
            lap_times: (0..n).map(|_| Some(self.u16()? as f32 / 100.0)).collect::<Option<_>>()?,
            finished: self.time()?,
            armed: self.bool()?,
            lap_fraction: self.unit()?,
            dnf: self.bool()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn racer() -> RacerState {
        let wheel = WheelState { compression: 0.12, contact: true, contact_normal: Vec3::Y, spin: 2.5, steer: -0.3, surface: 4 };
        RacerState {
            quad: QuadPose {
                position: Vec3::new(120.5, 3.25, -880.125),
                rotation: Quat::from_rotation_y(0.7),
                velocity: Vec3::new(10.0, -1.5, 22.0),
                angular_velocity: Vec3::new(0.25, -1.0, 0.5),
                wheels: vec![wheel; 4],
                steer: -0.5,
                throttle: 1.0,
                brake: 0.0,
                lean: 0.25,
                gear: 3,
                revs: 6500.0,
                preload: 0.5,
                air_time: 0.75,
            },
            lean: -1.0,
            boost: Boost { meter: 0.4, on: true },
            kick: Some((0.25, true)),
            flinch: None,
            trick: Some((7, 0.5)),
        }
    }

    fn near(a: &RacerState, b: &RacerState) -> bool {
        let (p, q) = (&a.quad, &b.quad);
        p.position == q.position
            && p.rotation.dot(q.rotation).abs() > 0.9999
            && p.velocity == q.velocity
            && p.angular_velocity.distance(q.angular_velocity) < 0.01
            && p.wheels.iter().zip(&q.wheels).all(|(u, v)| (u.compression - v.compression).abs() < 1e-3 && (u.spin - v.spin).abs() < 1e-3 && u.contact == v.contact && u.surface == v.surface)
            && (p.steer - q.steer).abs() < 1e-3
            && (p.throttle, p.gear, p.revs, p.air_time) == (q.throttle, q.gear, q.revs, q.air_time)
            && (a.lean - b.lean).abs() < 1e-3
            && a.boost == b.boost
            && a.kick.zip(b.kick).is_some_and(|(x, y)| (x.0 - y.0).abs() < 1e-3 && x.1 == y.1)
            && a.flinch == b.flinch
            && a.trick == b.trick
    }

    #[test]
    fn messages_come_back_as_sent() {
        let report = Report { clock: 75.5, lap_times: vec![61.25, 59.5], finished: None, armed: true, lap_fraction: 0.6, dnf: false };
        let seats = vec![Seat { id: RacerId(0), rider: "Male1".into(), quad: "Bandit".into(), human: true }, Seat { id: RacerId(3), rider: "Female2".into(), quad: "Blaze".into(), human: false }];
        for m in [
            Msg::Hello { retail: true, rider: "Male1".into(), quad: "Rocket".into() },
            Msg::Refused("the race has started".into()),
            Msg::Lobby { track: "Dunes1".into(), laps: 3, players: vec!["KEITH LITTLE".into(), "BUDDY FOX".into()] },
            Msg::Race { you: RacerId(2), track: "Swamp2".into(), laps: 5, seats },
            Msg::Ready,
            Msg::Bye,
        ] {
            assert_eq!(Msg::decode(&m.encode()), Some(m));
        }
        let Some(Msg::Snapshot { time, go, racers }) = Msg::decode(&Msg::Snapshot { time: 12.5, go: true, racers: vec![(RacerId(4), racer(), report.clone())] }.encode()) else { panic!() };
        assert_eq!((time, go, racers.len(), racers[0].0), (12.5, true, 1, RacerId(4)));
        assert!(near(&racers[0].1, &racer()));
        assert_eq!(racers[0].2.lap_times, report.lap_times);
        assert!((racers[0].2.lap_fraction - 0.6).abs() < 0.01 && racers[0].2.armed && racers[0].2.finished.is_none());
        let Some(Msg::Mine { state, .. }) = Msg::decode(&Msg::Mine { time: 1.0, state: racer(), report }.encode()) else { panic!() };
        assert!(near(&state, &racer()));
    }

    #[test]
    fn a_full_grid_fits_a_datagram() {
        let report = Report { lap_times: vec![60.0; 9], ..Report::default() };
        let racers = (0..8).map(|i| (RacerId(i), racer(), report.clone())).collect();
        assert!(Msg::Snapshot { time: 0.0, go: true, racers }.encode().len() < 1400);
    }

    #[test]
    fn other_builds_and_junk_are_ignored() {
        let mut b = Msg::Ready.encode();
        b[4] ^= 0xff;
        assert_eq!(Msg::decode(&b), None);
        assert_eq!(Msg::decode(b"hello"), None);
        let mut b = Msg::Lobby { track: "Dunes1".into(), laps: 3, players: vec![] }.encode();
        b.truncate(b.len() - 2);
        assert_eq!(Msg::decode(&b), None);
    }
}
