//! Split-screen multiplayer: two to four players racing on one screen.
//!
//! As in the original, two players split the screen top and bottom and
//! three or four take a quarter each (the fourth stays empty for three),
//! and computer riders make the field up to four. Player 1 drives the
//! usual player quad; players 2 to 4 drive racer quads (see
//! [`opponents::Racer::human`]), each with their own keys or pad and their
//! own view. The race is over once every player is home, or a minute
//! after the first one.
//!
//! Freestyle Battle is Open Freestyle for the same players with no
//! computer riders: each player has their own trick session, and the most
//! points when the time runs out wins.
//!
//! The Championship is the original's custom championship for the same
//! players: they choose its tracks and laps (and, once Custom is open in
//! the career, the points split), computer riders from the Custom field
//! make it up to four, and each round scores both the race and the trick
//! ranking as a career round does.
//!
//! As in the original, the championship is the profile's own: the
//! original's multiplayer Championship takes over the profile's one
//! championship (its `CareerData`, which 0x97850 resets and sets to Custom,
//! and 0x11cb0 saves), so starting one replaces a career championship in
//! progress. It's saved after each round. Multiplayer Championship always
//! starts afresh; the saved one carries on from Career's Continue, where
//! the profile only knows the field's riders, so player 1 races on and the
//! other players' riders race as computer riders.
//!
//! [`opponents::Racer::human`]: crate::opponents::Racer::human

use bevy::camera::Viewport;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::career::{self, Career, Entrant, Plan, Run};
use crate::freestyle::Session;
use crate::frontend::Roster;
use crate::hud::ordinal;
use crate::opponents::{Opponents, Standings};
use crate::race::{fmt_time, RaceProgress};
use crate::ui::{set_text, BitmapText, FontId};
use crate::AppState;

pub const MAX_PLAYERS: usize = 4;
/// Riders in a split-screen race, players included.
pub const FIELD: usize = 4;
/// How long the others get once the first player is home.
pub const WAIT_SECS: f32 = 60.0;

/// The view of player `0` (0-based) in split screen.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Split(pub usize);

/// The camera the race menus draw on in split screen, over every view.
#[derive(Component)]
pub struct Overlay;

/// Which part of the window (x, y, width, height, as shares of it) view
/// `slot` takes with `players` playing.
pub fn rect(slot: usize, players: usize) -> (f32, f32, f32, f32) {
    match players {
        0 | 1 => (0.0, 0.0, 1.0, 1.0),
        2 => (0.0, slot as f32 * 0.5, 1.0, 0.5),
        _ => ((slot % 2) as f32 * 0.5, (slot / 2) as f32 * 0.5, 0.5, 0.5),
    }
}

/// Riders and quads for players 2 onwards when none were picked (from the
/// command line): the next riders and quads in the roster.
pub fn default_picks(roster: &Roster, rider: &str, quad: &str, players: usize) -> Vec<(String, String)> {
    let riders = roster.riders.iter().map(|r| r.model.clone()).filter(|m| !m.eq_ignore_ascii_case(rider));
    let mut quads = roster.quads.iter().map(|q| q.name.clone()).filter(|q| !q.eq_ignore_ascii_case(quad)).cycle();
    riders.take(players.saturating_sub(1)).map(|r| (r, quads.next().unwrap_or_else(|| quad.to_string()))).collect()
}

/// The views for players 2 onwards, and the overlay for the menus.
pub fn spawn_views(commands: &mut Commands, view: impl Bundle + Clone, players: usize, scoped: &DespawnOnExit<AppState>) {
    for slot in 1..players.min(MAX_PLAYERS) {
        commands.spawn((
            view.clone(),
            // Player 1's view clears the window; the rest draw over it.
            Camera { order: slot as isize, clear_color: ClearColorConfig::None, ..default() },
            Transform::default(),
            crate::ChaseCam { offset: Vec3::new(0.0, 2.5, 6.0), facing: Quat::IDENTITY },
            Split(slot),
            scoped.clone(),
        ));
    }
    commands.spawn((Camera2d, Camera { order: 10, clear_color: ClearColorConfig::None, ..default() }, Overlay, scoped.clone()));
}

/// Freestyle Battle: each player's (1-based) score.
pub fn battle_scores(p1: &Session, opp: &Opponents) -> Vec<(usize, u32)> {
    std::iter::once((1, p1.score)).chain(opp.racers.iter().filter_map(|r| Some((r.human?, r.session.as_ref()?.score)))).collect()
}

/// Player `n`'s place by score (players level share a place).
pub fn place(scores: &[(usize, u32)], n: usize) -> usize {
    let mine = scores.iter().find(|s| s.0 == n).map_or(0, |s| s.1);
    1 + scores.iter().filter(|s| s.1 > mine).count()
}

/// A split-screen championship in progress. The field is the players in
/// order (marked `player`), then the computer riders. The profile keeps a
/// copy (see [`MpChamp::keep`]).
#[derive(Resource, Default)]
pub struct MpChamp {
    pub run: Option<Run>,
}

impl MpChamp {
    /// `players`: each player's (name, model); `pool`: the computer riders
    /// to draw from.
    pub fn start(&mut self, plan: Plan, players: &[(String, String)], quad: &str, pool: &[(String, String)], seed: u32) {
        let mut seed = seed | 1;
        let mut pool: Vec<&(String, String)> = pool.iter().filter(|(_, m)| !players.iter().any(|p| p.1.eq_ignore_ascii_case(m))).collect();
        let mut field: Vec<Entrant> = players
            .iter()
            .enumerate()
            .map(|(i, (name, model))| Entrant { name: format!("P{} {name}", i + 1), model: model.clone(), points: 0, player: true })
            .collect();
        while field.len() < FIELD && !pool.is_empty() {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let (name, model) = pool.swap_remove(seed as usize % pool.len());
            field.push(Entrant { name: name.clone(), model: model.clone(), points: 0, player: false });
        }
        let rider = players.first().map_or(String::new(), |p| p.1.clone());
        self.run = Some(Run::new(plan, rider, quad.to_string(), field));
    }

    /// Scores a finished round: `order` is field indices, winner first;
    /// `tricks` each entrant's trick score, by field index (most tricks
    /// first, finishing order breaking ties, as a career round).
    pub fn score(&mut self, rules: &career::Rules, order: &[usize], tricks: &[u32]) {
        let Some(run) = self.run.as_mut().filter(|r| !r.over()) else { return };
        let (race, trick) = run.points(rules);
        let mut by_tricks = order.to_vec();
        by_tricks.sort_by_key(|&i| std::cmp::Reverse(tricks.get(i).copied().unwrap_or(0)));
        for (table, ranking) in [(race, order), (trick, by_tricks.as_slice())] {
            for (place, &i) in ranking.iter().enumerate() {
                if let Some(e) = run.field.get_mut(i) {
                    e.points += table.get(place).copied().unwrap_or(0);
                }
            }
        }
        run.round += 1;
    }

    /// The championship as the profile saves it: player 1 is the player,
    /// the other players' riders computer riders, by their riders' names.
    pub fn saved(&self) -> Option<Run> {
        let mut run = self.run.clone()?;
        for (i, e) in run.field.iter_mut().enumerate().filter(|(_, e)| e.player) {
            if let Some(name) = e.name.strip_prefix(&format!("P{} ", i + 1)) {
                e.name = name.to_string();
            }
            e.player = i == 0;
        }
        Some(run)
    }

    /// Makes it the profile's championship and saves the profile.
    pub fn keep(&self, career: &mut Career) {
        if let Some(run) = self.saved() {
            career.run = Some(run);
            career.save();
        }
    }
}

/// Whether every player is home, or the others have had long enough.
pub fn all_home(p1: &RaceProgress, opponents: Option<&Opponents>) -> bool {
    let Some(opp) = opponents else { return p1.over() };
    let humans: Vec<&RaceProgress> = std::iter::once(p1).chain(opp.racers.iter().filter(|r| r.human.is_some()).map(|r| &r.progress)).collect();
    if humans.iter().all(|p| p.over()) {
        return true;
    }
    // The first finish, on the race clock (everyone's clock runs alike).
    let first = humans.iter().filter_map(|p| p.finished).reduce(f32::min);
    first.is_some_and(|f| humans.iter().map(|p| p.clock).fold(0.0, f32::max) > f + WAIT_SECS)
}

pub struct MpPlugin;

impl Plugin for MpPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MpChamp>().add_systems(Update, (split_views, spawn_hud.run_if(resource_added::<Opponents>), update_hud).chain().run_if(in_state(AppState::Race)));
    }
}

/// Keeps each view on its part of the window.
fn split_views(window: Query<&Window, With<PrimaryWindow>>, mut cams: Query<(&mut Camera, &Split)>) {
    let Ok(window) = window.single() else { return };
    let size = window.physical_size().as_vec2();
    let players = cams.iter().count();
    for (mut cam, split) in &mut cams {
        let (x, y, w, h) = rect(split.0, players);
        let vp = Viewport {
            physical_position: (Vec2::new(x, y) * size).as_uvec2(),
            physical_size: (Vec2::new(w, h) * size).as_uvec2().max(UVec2::ONE),
            ..default()
        };
        if cam.viewport.as_ref().is_none_or(|v| v.physical_position != vp.physical_position || v.physical_size != vp.physical_size) {
            cam.viewport = Some(vp);
        }
    }
}

#[derive(Component)]
enum Readout {
    Lap(usize),
    Place(usize),
    Time(usize),
    Speed(usize),
    Banner(usize),
}

/// A HUD on each other player's view: lap, place, time, speed. Player 1
/// has the usual one on theirs.
fn spawn_hud(mut commands: Commands, opponents: Res<Opponents>, cams: Query<(Entity, &Split)>) {
    for (cam, split) in &cams {
        let n = split.0 + 1;
        if n == 1 || opponents.human(n).is_none() {
            continue;
        }
        commands
            .spawn((crate::ui::fullscreen(), UiTargetCamera(cam), DespawnOnExit(AppState::Race), Pickable::IGNORE))
            .with_children(|root| {
                root.spawn(Node { position_type: PositionType::Absolute, left: px(16), top: px(10), flex_direction: FlexDirection::Column, row_gap: px(3), ..default() })
                    .with_children(|c| {
                        c.spawn(BitmapText::new(FontId::Small, 9.0, format!("PLAYER {n}")).color(Color::srgb(1.0, 0.85, 0.3)));
                        c.spawn((BitmapText::new(FontId::Chrome, 18.0, ""), Readout::Lap(n)));
                        c.spawn((BitmapText::new(FontId::Chrome, 14.0, ""), Readout::Place(n)));
                    });
                root.spawn(Node { position_type: PositionType::Absolute, right: px(16), top: px(10), ..default() })
                    .with_child((BitmapText::new(FontId::Chrome, 16.0, ""), Readout::Time(n)));
                root.spawn(Node { position_type: PositionType::Absolute, right: px(16), bottom: px(10), ..default() })
                    .with_child((BitmapText::new(FontId::Chrome, 16.0, ""), Readout::Speed(n)));
                root.spawn(Node { position_type: PositionType::Absolute, width: percent(100), top: percent(40), justify_content: JustifyContent::Center, ..default() })
                    .with_child((BitmapText::new(FontId::Menu, 18.0, ""), Readout::Banner(n)));
            });
    }
}

fn update_hud(
    opponents: Option<Res<Opponents>>,
    standings: Option<Res<Standings>>,
    p1: Option<Res<RaceProgress>>,
    session: Option<Res<Session>>,
    mut readouts: Query<(&Readout, &mut BitmapText)>,
) {
    let (Some(opp), Some(p1)) = (opponents, p1) else { return };
    let battle = session.as_deref().filter(|s| s.freestyle);
    let scores = battle.map(|s| battle_scores(s, &opp)).unwrap_or_default();
    for (r, mut t) in &mut readouts {
        let n = match r {
            Readout::Lap(n) | Readout::Place(n) | Readout::Time(n) | Readout::Speed(n) | Readout::Banner(n) => *n,
        };
        let Some((i, racer)) = opp.racers.iter().enumerate().find(|(_, r)| r.human == Some(n)) else { continue };
        let p = &racer.progress;
        if let (Some(p1s), Some(s)) = (battle, racer.session.as_ref()) {
            let text = match r {
                Readout::Lap(_) => format!("SCORE {}", s.score),
                Readout::Place(_) => format!("{} OF {}", ordinal(place(&scores, n)), scores.len()),
                Readout::Time(_) => fmt_time(p1s.time_left(p1.clock).unwrap_or(p1.clock)),
                Readout::Speed(_) => format!("{:.0} MPH", racer.vehicle.speed_mph().abs()),
                Readout::Banner(_) if p1s.over(p1.clock) => "TIME UP".into(),
                // The latest trick call-out.
                Readout::Banner(_) => s.popups.last().filter(|pp| s.time - pp.at < 2.2).map_or(String::new(), |pp| pp.text.clone()),
            };
            set_text(&mut t, &text);
            continue;
        }
        let text = match r {
            Readout::Lap(_) => format!("LAP {}/{}", p.lap(), p.laps),
            Readout::Place(_) => standings.as_ref().and_then(|s| s.opponents.get(i)).map_or(String::new(), |o| format!("{} OF {}", ordinal(o.0), opp.racers.len() + 1)),
            Readout::Time(_) => fmt_time(p.finished.unwrap_or(p.clock)),
            Readout::Speed(_) => format!("{:.0} MPH", racer.vehicle.speed_mph().abs()),
            Readout::Banner(_) if !p.started() => String::new(),
            Readout::Banner(_) if p.finished.is_some() && !p1.over() => "WAITING FOR OTHER COMPETITORS".into(),
            Readout::Banner(_) if p.finished.is_some() => "FINISHED".into(),
            Readout::Banner(_) => String::new(),
        };
        set_text(&mut t, &text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts() {
        // Two: top and bottom; three or four: quarters.
        assert_eq!(rect(0, 2), (0.0, 0.0, 1.0, 0.5));
        assert_eq!(rect(1, 2), (0.0, 0.5, 1.0, 0.5));
        assert_eq!(rect(1, 3), (0.5, 0.0, 0.5, 0.5));
        assert_eq!(rect(3, 4), (0.5, 0.5, 0.5, 0.5));
    }

    #[test]
    fn championship_scores_rounds() {
        let rules = career::Rules::default();
        let mut c = MpChamp::default();
        let players = [("ANN".to_string(), "Female1".to_string()), ("BOB".to_string(), "Male1".to_string())];
        let pool: Vec<(String, String)> = (2..8).map(|i| (format!("R{i}"), format!("Male{i}"))).collect();
        let plan = Plan { champ: career::Champ::Amateur, schedule: vec![("Dunes1".into(), career::LAPS); career::ROUNDS], split: None };
        c.start(plan, &players, "Bandit", &pool, 7);
        let run = c.run.as_ref().unwrap();
        assert_eq!(run.field.len(), FIELD);
        assert_eq!(run.field[1].name, "P2 BOB");
        assert!(run.field[1].player && !run.field[2].player);
        // P2 wins the race, P1 throws the most tricks.
        let mut tricks = [0; FIELD];
        tricks[0] = 900;
        c.score(&rules, &[1, 0, 2, 3], &tricks);
        let run = c.run.as_ref().unwrap();
        assert_eq!(run.round, 1);
        assert_eq!((run.field[0].points, run.field[1].points, run.field[2].points), (20 + 25, 25 + 20, 16 + 16));
    }

    #[test]
    fn championship_saves_as_the_profiles() {
        let rules = career::Rules::default();
        let mut c = MpChamp::default();
        let players = [("ANN".to_string(), "Female1".to_string()), ("BOB".to_string(), "Male1".to_string())];
        let pool: Vec<(String, String)> = (2..8).map(|i| (format!("R{i}"), format!("Male{i}"))).collect();
        let plan = Plan { champ: career::Champ::Custom, schedule: vec![("Dunes1".into(), 2), ("Swamp1".into(), 4)], split: None };
        c.start(plan, &players, "Bandit", &pool, 7);
        c.score(&rules, &[1, 0, 2, 3], &[0; FIELD]);
        let run = c.saved().unwrap();
        // Player 1 carries on; player 2's rider races as a computer rider.
        assert_eq!((run.field[0].name.as_str(), run.field[0].player), ("ANN", true));
        assert_eq!((run.field[1].name.as_str(), run.field[1].player), ("BOB", false));
        assert_eq!(run.opponents()[0], "Male1");
        assert_eq!((run.rider.as_str(), run.quad.as_str(), run.round), ("Female1", "Bandit", 1));
        // And it comes back from the profile as it went in.
        let mut career = Career::new();
        career.run = Some(run.clone());
        assert_eq!(Career::parse(&career.to_text()).run, Some(run));
    }

    #[test]
    fn battle_places() {
        let scores = [(1, 500), (2, 900), (3, 500)];
        assert_eq!(place(&scores, 2), 1);
        assert_eq!(place(&scores, 1), 2);
        assert_eq!(place(&scores, 3), 2);
    }

    #[test]
    fn waits_for_everyone_then_a_minute() {
        let mut p1 = RaceProgress::new(1, true);
        p1.countdown = 0.0;
        assert!(!all_home(&p1, None));
        p1.finished = Some(90.0);
        assert!(all_home(&p1, None));
    }
}
