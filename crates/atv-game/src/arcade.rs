//! Arcade's field, as the original's (`ArcadeSM` at 0x95e70; the field at
//! 0x1dd200, kept in the profile as `mRound`, `Player%d` and `mDropOuts%d`,
//! 0x11470): the player and seven computer riders.
//!
//! - A new game puts the player at the back of the grid (0x11530).
//! - After a round is won the grid is the finishing order, and the last
//!   three are out (0x115b0). Three new riders take their places at the
//!   back, drawn from the computer riders ranked nearest the next round's
//!   difficulty: the round / 14, raised by 40% below 0.6 (0x11130,
//!   0x9c090). A rider who went out sits out the next two draws.
//! - The Arcade Status page before each round shows the grid, each rider's
//!   ranking and who went out (0x38ee0).
//! - Arcade is 15 rounds: winning the fifteenth brings up Arcade Complete
//!   instead of a sixteenth (0x96070). The disc's `Arcade.xml` lists a
//!   sixteenth round, which the original never races.

use crate::career::Rules;
use crate::frontend::{RiderInfo, Roster};

/// Riders in an Arcade round, the player included.
pub const FIELD: usize = 8;
/// The rounds the original races.
pub const ROUNDS: usize = 15;
/// The places that go out after a round.
const OUT: usize = 3;
/// Draws a rider who went out sits out.
const SIT_OUT: u32 = 2;

/// A computer rider in the field.
#[derive(Clone, Debug, PartialEq)]
pub struct Entrant {
    pub model: String,
    /// Grid place, 0 at the front.
    pub grid: usize,
    /// Rounds raced in the field (0: new this round).
    pub rounds: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArcadeField {
    pub riders: Vec<Entrant>,
    /// The player's grid place.
    pub grid: usize,
    /// Riders out of the field, and the draws since (1: out in the last
    /// round).
    pub out: Vec<(String, u32)>,
}

impl ArcadeField {
    /// A new field, the player at the back: round 1's, or `round`'s for a
    /// save from before fields were kept.
    pub fn new(roster: &Roster, player: &str, round: usize) -> Self {
        let picks = draw(roster, |m| m.eq_ignore_ascii_case(player), difficulty(round), FIELD - 1);
        let riders = picks.into_iter().enumerate().map(|(i, model)| Entrant { model, grid: FIELD - 2 - i, rounds: 0 }).collect();
        Self { riders, grid: FIELD - 1, out: Vec::new() }
    }

    /// The computer riders front to back, the order the race's grid takes.
    pub fn grid_order(&self) -> Vec<String> {
        let mut r: Vec<&Entrant> = self.riders.iter().collect();
        r.sort_by_key(|e| e.grid);
        r.into_iter().map(|e| e.model.clone()).collect()
    }

    /// The player's slot on the track's grid (1 at the front).
    pub fn player_slot(&self) -> usize {
        self.riders.iter().filter(|e| e.grid < self.grid).count() + 1
    }

    /// Swaps a computer rider out for another if the player now rides them
    /// (picked on rider select after the field was drawn).
    pub fn without(&mut self, roster: &Roster, player: &str, round: usize) {
        let Some(i) = self.riders.iter().position(|e| e.model.eq_ignore_ascii_case(player)) else { return };
        let taken = |m: &str| m.eq_ignore_ascii_case(player) || self.taken(m);
        match draw(roster, taken, difficulty(round), 1).pop() {
            Some(model) => self.riders[i].model = model,
            None => {
                self.riders.remove(i);
            }
        }
    }

    fn taken(&self, model: &str) -> bool {
        self.riders.iter().any(|e| e.model.eq_ignore_ascii_case(model)) || self.out.iter().any(|o| o.0.eq_ignore_ascii_case(model))
    }

    /// A round won: the grid becomes the finishing order. `place`: each
    /// computer rider's place (1-based), none if they didn't finish; the
    /// player won it.
    pub fn regrid(&mut self, place: impl Fn(&str) -> Option<usize>) {
        let mut order: Vec<(usize, usize, Option<usize>)> = vec![(1, self.grid, None)];
        for (i, e) in self.riders.iter().enumerate() {
            order.push((place(&e.model).unwrap_or(100), e.grid, Some(i)));
        }
        // Ties (riders not home) keep their grid order.
        order.sort_by_key(|o| (o.0, o.1));
        for (g, (_, _, who)) in order.into_iter().enumerate() {
            match who {
                Some(i) => {
                    self.riders[i].grid = g;
                    self.riders[i].rounds += 1;
                }
                None => self.grid = g,
            }
        }
    }

    /// Then the last three go out and new riders take their places at
    /// the back, drawn for `round` (the next).
    pub fn knock_out(&mut self, roster: &Roster, player: &str, round: usize) {
        for o in &mut self.out {
            o.1 += 1;
        }
        self.out.retain(|o| o.1 <= SIT_OUT);
        let (mut gone, stay): (Vec<Entrant>, Vec<Entrant>) = std::mem::take(&mut self.riders).into_iter().partition(|e| e.grid >= FIELD - OUT);
        gone.sort_by_key(|e| e.grid);
        self.riders = stay;
        self.out.extend(gone.into_iter().map(|e| (e.model, 1)));
        let used: Vec<usize> = self.riders.iter().map(|e| e.grid).chain([self.grid]).collect();
        let free = (0..FIELD).filter(|g| !used.contains(g));
        let want = (FIELD - 1).saturating_sub(self.riders.len());
        let picks = draw(roster, |m| m.eq_ignore_ascii_case(player) || self.taken(m), difficulty(round), want);
        for (model, grid) in picks.into_iter().zip(free) {
            self.riders.push(Entrant { model, grid, rounds: 0 });
        }
    }

    /// The riders who went out after the last round.
    pub fn just_out(&self) -> Vec<&str> {
        self.out.iter().filter(|o| o.1 == 1).map(|o| o.0.as_str()).collect()
    }
}

/// How strong a round's new riders are, 0 to 1 (0x11130).
pub fn difficulty(round: usize) -> f32 {
    let d = round as f32 / 14.0;
    let d = if d < 0.6 { d * 1.4 } else { d };
    d.clamp(0.0, 1.0)
}

/// `count` computer riders ranked nearest `difficulty` (0 the weakest),
/// skipping `taken`: the one at that rank, then one below, one above and
/// so on (0x9c090).
fn draw(roster: &Roster, taken: impl Fn(&str) -> bool, difficulty: f32, count: usize) -> Vec<String> {
    let mut pool: Vec<&RiderInfo> = roster.riders.iter().filter(|r| r.ai).collect();
    pool.sort_by(|a, b| (a.skill, a.stats[0]).partial_cmp(&(b.skill, b.stats[0])).unwrap_or(std::cmp::Ordering::Equal));
    let n = pool.len() as isize;
    let centre = (difficulty * (n - 1).max(0) as f32).round() as isize;
    let order = std::iter::once(centre).chain((1..n.max(1)).flat_map(|d| [centre - d, centre + d]));
    let mut picks: Vec<String> = Vec::new();
    for i in order {
        if picks.len() >= count {
            break;
        }
        let Some(r) = usize::try_from(i).ok().and_then(|i| pool.get(i)) else { continue };
        if !taken(&r.model) && !picks.iter().any(|p| p.eq_ignore_ascii_case(&r.model)) {
            picks.push(r.model.clone());
        }
    }
    picks
}

/// A computer rider's ranking on Arcade Status: the certificate their
/// skill rating reaches (0x88470).
pub fn ranking(rules: &Rules, rider: &RiderInfo) -> &'static str {
    rules.certificate_at(usize::from(rider.skill))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roster() -> Roster {
        let mut r = Roster::default();
        r.riders = (0..20)
            .map(|i| RiderInfo {
                name: format!("RIDER {i}"),
                model: format!("Male{i}"),
                born: String::new(),
                from: String::new(),
                stats: [i as f32 / 4.0; 4],
                ai: true,
                locked: false,
                skill: (1 + i / 3) as u8,
            })
            .collect();
        r
    }

    #[test]
    fn rounds_get_harder() {
        assert_eq!(difficulty(0), 0.0);
        assert!((difficulty(5) - 0.5).abs() < 1e-6);
        assert_eq!(difficulty(14), 1.0);
        let roster = roster();
        let f = ArcadeField::new(&roster, "Male0", 0);
        // The weakest riders, the player at the back and not among them.
        assert_eq!(f.grid, FIELD - 1);
        assert_eq!(f.player_slot(), FIELD);
        assert_eq!(f.riders.len(), FIELD - 1);
        assert!(f.riders.iter().all(|e| e.model != "Male0" && e.grid < FIELD - 1));
        assert!(f.riders.iter().all(|e| e.model.trim_start_matches("Male").parse::<usize>().unwrap() <= 8));
    }

    #[test]
    fn last_three_go_out() {
        let roster = roster();
        let mut f = ArcadeField::new(&roster, "Male0", 0);
        let order = f.grid_order();
        // The computer riders finish in grid order behind the player.
        f.regrid(|m| order.iter().position(|o| o == m).map(|i| i + 2));
        assert_eq!(f.grid, 0);
        assert_eq!(f.player_slot(), 1);
        f.knock_out(&roster, "Male0", 7);
        assert_eq!(f.just_out(), order[4..].iter().map(String::as_str).collect::<Vec<_>>());
        assert_eq!(f.riders.len(), FIELD - 1);
        let new: Vec<&Entrant> = f.riders.iter().filter(|e| e.rounds == 0).collect();
        assert_eq!(new.len(), 3);
        assert!(new.iter().all(|e| e.grid >= FIELD - 3 && !order.contains(&e.model)));
        let mut grids: Vec<usize> = f.riders.iter().map(|e| e.grid).chain([f.grid]).collect();
        grids.sort();
        assert_eq!(grids, (0..FIELD).collect::<Vec<_>>());
        // Out for the next two draws, then back in the pool.
        let gone = &order[4..];
        f.regrid(|_| None);
        f.knock_out(&roster, "Male0", 7);
        assert!(f.riders.iter().all(|e| !gone.contains(&e.model)));
        assert!(gone.iter().all(|g| f.out.contains(&(g.clone(), 2))));
        f.regrid(|_| None);
        f.knock_out(&roster, "Male0", 7);
        assert!(f.out.iter().all(|o| !gone.contains(&o.0)));
    }
}
