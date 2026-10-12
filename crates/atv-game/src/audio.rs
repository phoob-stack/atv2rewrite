//! Sound: each environment's music and ambience, and the player's engine
//! and wheel noise, all decoded from the player's own copy of the game.
//!
//! `Setup/Sound.xml` gives the mix levels, the music sections and the
//! ambient banks. The engine follows the original's update (0x93c60): two
//! looping Vehicles-bank samples, "on" (2) and "off" (1), whose volume
//! and pitch track the revs and throttle. Every other quad on the track
//! runs the same two loops from its own revs, quieter the further it is
//! from the nearest player.
//!
//! Split screen: the original gives every player the same set of sounds
//! (0x92ff0 makes one per player: engine, knocks, voice and wheel noise)
//! and places each 3D sound against the listener of the nearest view
//! (0x76180 picks it with 0x75e50), so every player hears their own quad
//! as loud as player 1 hears theirs, and other quads by how close they
//! are to any player. With three or four players each quad's wheel noise
//! comes from its first wheel only, where one or two get a voice per
//! wheel.
//!
//! One-shots, as the original plays them: the SpotFX bank in the menus
//! (0 cursor, 1 select or back) at `mFrontEndSpotFXVol`; a Riders-bank
//! grunt when a thrown rider hits the ground hard (0x94690: female clips
//! 0-13, male 14-32, never the same twice running); and a Vehicles-bank
//! knock when the quad's body hits something (0x94480).
//!
//! The retail disc rebuilt the Vehicles and Riders banks, so the same clip
//! numbers are other sounds there: its engine loops are 0 ("off") and 1
//! ("on"), and its wheel, knock and rider clips sit elsewhere too. With the
//! retail Sound.xml (mix levels per platform) the indices, the engine mix
//! and the one-shot rules come from the retail executable instead (XBE
//! addresses: engine 0xb1690, knocks 0xb1e90, rider 0xaed30).
//!
//! Race music on the retail disc: besides each area's track it has eight
//! licensed songs (`Setup/Music.xml`, `/Licensed/Track1-8.wav`), and its
//! race start (0x9f5c0) plays what the profile's Playlist choice says:
//! the area's track, the songs in playlist order, the songs shuffled (the
//! default), or nothing. See [`Playlist`] and [`Jukebox`].

use std::sync::Arc;

use bevy::audio::{AddAudioSource, ChannelCount, Decodable, SampleRate, Source, Volume};
use bevy::prelude::*;

use atv_formats::music::{self, Song};
use atv_formats::sound::SoundSetup;
use atv_formats::wav::{self, Pcm, Stream};
use atv_formats::xwb::WaveBank;
use atv_formats::GameSource;

use crate::settings::{gain, Settings};
use crate::{AppState, Game, Mode, Player, Selection};

pub struct AudioPlugin;

impl Plugin for AudioPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<Sound>()
            .add_audio_source::<Sound>()
            .init_asset::<Songs>()
            .add_audio_source::<Songs>()
            .add_systems(PreUpdate, load_banks.run_if(resource_exists::<Game>).run_if(not(resource_exists::<Banks>)))
            // After the race is set up, so the other quads are there to hear.
            .add_systems(OnEnter(AppState::Race), start_race_audio.after(crate::setup))
            .add_systems(Update, (engine_and_wheels, crash_sounds, bed_volumes).run_if(in_state(AppState::Race)))
            .add_systems(Update, menu_sounds.run_if(in_state(AppState::FrontEnd)))
            .add_systems(Update, pause_menu_sounds.run_if(in_state(AppState::Race)));
    }
}

/// Vehicles bank samples (the name table at 0x1d2838 covers 3..16).
const ENGINE_OFF: usize = 1;
const ENGINE_ON: usize = 2;
const VEHICLE_SAMPLES: [(&str, usize); 14] = [
    ("Gravel1", 3),
    ("Gravel2", 4),
    ("Gravel3", 5),
    ("Rumble1", 6),
    ("Rumble2", 7),
    ("Rumble3", 8),
    ("Rumble4", 9),
    ("Water1", 10),
    ("Water2", 11),
    ("Water3", 12),
    ("Twigs1", 13),
    ("Twigs2", 14),
    ("Screech1", 15),
    ("Screech2", 16),
];
/// The retail Vehicles bank (30 clips): the engine loops are first (the
/// engine part 0xb2150 plays 1 as "on" and 0 as "off"), and the wheel
/// names are the table at 0x1ce638.
const RETAIL_ENGINE_OFF: usize = 0;
const RETAIL_ENGINE_ON: usize = 1;
const RETAIL_VEHICLE_SAMPLES: [(&str, usize); 10] = [
    ("Gravel1", 2),
    ("Rumble1", 3),
    ("Rumble2", 4),
    ("Rumble3", 5),
    ("Rumble4", 6),
    ("Water1", 7),
    ("Water2", 8),
    ("Twigs1", 9),
    ("Twigs2", 10),
    ("Screech1", 11),
];

/// A playable sound: clips played in order, then repeated from
/// `loop_from` (past the end means play once).
#[derive(Asset, TypePath, Clone)]
pub struct Sound {
    parts: Arc<[Arc<Pcm>]>,
    loop_from: usize,
}

impl Sound {
    fn looped(pcm: Arc<Pcm>) -> Self {
        Sound { parts: Arc::new([pcm]), loop_from: 0 }
    }
}

pub struct SoundDecoder {
    sound: Sound,
    part: usize,
    pos: usize,
}

impl Iterator for SoundDecoder {
    type Item = bevy::audio::Sample;

    fn next(&mut self) -> Option<Self::Item> {
        let parts = &self.sound.parts;
        // Bounded so a sound made only of empty clips ends.
        for _ in 0..=parts.len() {
            let pcm = parts.get(self.part)?;
            if let Some(&s) = pcm.samples.get(self.pos) {
                self.pos += 1;
                return Some(s as f32 / 32768.0);
            }
            self.pos = 0;
            self.part += 1;
            if self.part >= parts.len() {
                self.part = self.sound.loop_from;
            }
        }
        None
    }
}

impl Source for SoundDecoder {
    fn current_span_len(&self) -> Option<usize> {
        // Clips may differ in format, so each is its own span.
        let pcm = self.sound.parts.get(self.part)?;
        Some(pcm.samples.len().saturating_sub(self.pos).max(1))
    }

    fn channels(&self) -> ChannelCount {
        let c = self.sound.parts.get(self.part).map_or(1, |p| p.channels);
        ChannelCount::new(c.max(1)).unwrap()
    }

    fn sample_rate(&self) -> SampleRate {
        let r = self.sound.parts.get(self.part).map_or(44100, |p| p.rate);
        SampleRate::new(r.max(1)).unwrap()
    }

    fn total_duration(&self) -> Option<std::time::Duration> {
        None
    }
}

impl Decodable for Sound {
    type Decoder = SoundDecoder;

    fn decoder(&self) -> Self::Decoder {
        SoundDecoder { sound: self.clone(), part: 0, pos: 0 }
    }
}

/// Sound.xml and the always-loaded banks, read once.
#[derive(Resource)]
struct Banks {
    setup: SoundSetup,
    vehicles: Vec<Arc<Pcm>>,
    riders: Vec<Arc<Pcm>>,
    spotfx: Vec<Arc<Pcm>>,
}

fn load_banks(mut commands: Commands, game: Res<Game>) {
    commands.insert_resource(Banks::load(&game.0));
    if let Some(j) = Jukebox::load(&game.0) {
        commands.insert_resource(j);
    }
}

/// The retail Playlist page's choice of race music (the profile's
/// `PlaylistOption0-3`, read by the race start 0x9f5c0).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Playlist {
    /// The area's own track (`ENVIRONMENT`).
    Environment,
    /// The licensed songs in playlist order, each race starting one song
    /// further on (`PLAYLIST`).
    InOrder,
    /// The licensed songs shuffled (`RANDOM`): a new profile's choice
    /// (0x13e40).
    #[default]
    Random,
    /// No race music (`MUTE_ALL`).
    Mute,
}

impl Playlist {
    pub const ALL: [Playlist; 4] = [Playlist::Environment, Playlist::InOrder, Playlist::Random, Playlist::Mute];

    pub fn key(self) -> &'static str {
        match self {
            Playlist::Environment => "environment",
            Playlist::InOrder => "playlist",
            Playlist::Random => "random",
            Playlist::Mute => "mute",
        }
    }

    pub fn parse(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.key() == key)
    }
}

/// The retail disc's licensed songs and the playlist state the retail game
/// keeps in the player's profile: the playlist order and the songs left
/// out of it (`Track0-7`, `mMusicTracksLockedOut`), and where the next
/// race starts in it (`mTrackInc`). Here they last for the session.
#[derive(Resource)]
pub struct Jukebox {
    songs: Vec<Song>,
    order: Vec<usize>,
    locked_out: u32,
    next: Option<usize>,
    /// Songs Training has played: each Training run gets one it hasn't,
    /// until all have (0xadfe0).
    played: u32,
    /// The disc only runs on North American consoles, where the retail
    /// game leaves Track5 out of every playlist and puts it last in the
    /// order (0x13290, 0x13770, 0xadfe0; the Playlist page 0x58c50 puts
    /// it back out whenever it's opened).
    na: bool,
    seed: u32,
}

/// The song North American consoles never play (Track5).
const NA_LEFT_OUT: usize = 4;

impl Jukebox {
    /// The songs, if the disc has them all (only the retail disc does).
    fn load(game: &GameSource) -> Option<Self> {
        let xml = game.read("/Setup/Music.xml").ok()?;
        let songs = music::parse(&String::from_utf8_lossy(&xml));
        if songs.is_empty() || songs.len() > 32 {
            return None;
        }
        if let Some(s) = songs.iter().find(|s| !game.exists(&s.path())) {
            warn!("licensed music: {} is missing, so races play the area music", s.path());
            return None;
        }
        let regions = game.read("/default.xbe").ok().and_then(|x| music::xbe_regions(&x));
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.subsec_nanos() ^ d.as_secs() as u32);
        Some(Self::new(songs, regions.is_some_and(|r| r & 7 == 1), seed))
    }

    fn new(songs: Vec<Song>, na: bool, seed: u32) -> Self {
        let na = na && songs.len() == 8;
        let order = if na { vec![0, 1, 2, 3, 5, 6, 7, NA_LEFT_OUT] } else { (0..songs.len()).collect() };
        let locked_out = if na { 1 << NA_LEFT_OUT } else { 0 };
        Jukebox { songs, order, locked_out, next: None, played: 0, na, seed }
    }

    fn rand(&mut self, n: usize) -> usize {
        self.seed = self.seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        (self.seed >> 16) as usize % n.max(1)
    }

    /// The songs in the playlist, in its order.
    fn enabled(&self) -> Vec<usize> {
        self.order.iter().copied().filter(|&i| self.locked_out & 1 << i == 0).collect()
    }

    /// What a race plays: `None` for the area's own track, else these
    /// songs one after another and round again (none: silence). As the
    /// retail race start 0x9f5c0: a challenge (game type 5) and the
    /// attract demo have no music, Training gets one song, and every other
    /// race goes by the Playlist choice (lists built by 0xaf360).
    fn race(&mut self, mode: Mode, demo: bool, choice: Playlist) -> Option<Vec<usize>> {
        if mode == Mode::Challenge || demo {
            return Some(Vec::new());
        }
        if mode == Mode::Training {
            return Some(self.training());
        }
        let songs = self.enabled();
        match choice {
            Playlist::Environment => None,
            Playlist::Mute => Some(Vec::new()),
            _ if songs.is_empty() => Some(Vec::new()),
            Playlist::InOrder => {
                // Each race starts one song further on (0x13fa0).
                let start = self.next.map_or(0, |n| (n + 1) % songs.len());
                self.next = Some(start);
                Some((0..songs.len()).map(|k| songs[(start + k) % songs.len()]).collect())
            }
            Playlist::Random => {
                let mut left = songs;
                let mut out = Vec::with_capacity(left.len());
                while !left.is_empty() {
                    let i = self.rand(left.len());
                    out.push(left.remove(i));
                }
                Some(out)
            }
        }
    }

    /// Training's one song (retail game type 12; ATV Academy here): any
    /// it hasn't played yet, looped.
    fn training(&mut self) -> Vec<usize> {
        let n = self.songs.len();
        if self.played.count_ones() as usize >= n {
            self.played = 0;
        }
        if self.na {
            self.played |= 1 << NA_LEFT_OUT;
        }
        let left: Vec<usize> = (0..n).filter(|&i| self.played & 1 << i == 0).collect();
        if left.is_empty() {
            return Vec::new();
        }
        let pick = left[self.rand(left.len())];
        self.played |= 1 << pick;
        vec![pick]
    }
}

/// Songs off the disc played one after another and round again, each
/// decoded as it plays (as the retail game streams its music, 0x896c0
/// picking what's next). The next song is read from the disc while one
/// plays.
#[derive(Asset, TypePath, Clone)]
pub struct Songs {
    game: Arc<GameSource>,
    paths: Arc<[String]>,
}

pub struct SongsDecoder {
    songs: Songs,
    index: usize,
    current: Option<Stream>,
    next: Option<std::sync::mpsc::Receiver<Option<Stream>>>,
}

fn open_song(game: &GameSource, path: &str) -> Option<Stream> {
    match game.read(path).map_err(|e| e.to_string()).and_then(|b| Stream::open(b).map_err(|e| e.to_string())) {
        Ok(s) => Some(s),
        Err(e) => {
            warn!("music {path}: {e}");
            None
        }
    }
}

impl SongsDecoder {
    /// Starts reading the song after the current one.
    fn fetch_next(&mut self) {
        let (tx, rx) = std::sync::mpsc::channel();
        let game = self.songs.game.clone();
        let path = self.songs.paths[(self.index + 1) % self.songs.paths.len()].clone();
        std::thread::spawn(move || {
            let _ = tx.send(open_song(&game, &path));
        });
        self.next = Some(rx);
    }
}

impl Iterator for SongsDecoder {
    type Item = bevy::audio::Sample;

    fn next(&mut self) -> Option<Self::Item> {
        // Bounded so a list whose songs all fail to open ends.
        for _ in 0..=self.songs.paths.len() {
            if let Some(s) = self.current.as_mut().and_then(Iterator::next) {
                return Some(s as f32 / 32768.0);
            }
            self.index = (self.index + 1) % self.songs.paths.len();
            self.current = self.next.take().and_then(|rx| rx.recv().ok().flatten());
            self.fetch_next();
        }
        None
    }
}

impl Source for SongsDecoder {
    fn current_span_len(&self) -> Option<usize> {
        // Songs may differ in format, so each is its own span.
        Some(self.current.as_ref().map_or(1, |s| s.remaining().max(1)))
    }

    fn channels(&self) -> ChannelCount {
        ChannelCount::new(self.current.as_ref().map_or(2, |s| s.channels).max(1)).unwrap()
    }

    fn sample_rate(&self) -> SampleRate {
        SampleRate::new(self.current.as_ref().map_or(48000, |s| s.rate).max(1)).unwrap()
    }

    fn total_duration(&self) -> Option<std::time::Duration> {
        None
    }
}

impl Decodable for Songs {
    type Decoder = SongsDecoder;

    fn decoder(&self) -> Self::Decoder {
        let mut d = SongsDecoder { current: self.paths.first().and_then(|p| open_song(&self.game, p)), songs: self.clone(), index: 0, next: None };
        if !self.paths.is_empty() {
            d.fetch_next();
        }
        d
    }
}

impl Banks {
    fn load(game: &GameSource) -> Self {
        let setup = game
            .read("/Setup/Sound.xml")
            .map(|b| SoundSetup::parse(&String::from_utf8_lossy(&b)))
            .unwrap_or_else(|e| {
                warn!("no Sound.xml: {e}");
                SoundSetup::default()
            });
        let vehicles = bank(game, "SOUND\\VEHICLES");
        let riders = bank(game, "SOUND\\RIDERS");
        let spotfx = bank(game, "SOUND\\SPOTFX");
        Banks { setup, vehicles, riders, spotfx }
    }
}

/// A sample bank's waves (`SOUND\DUNES` -> `/Sound/Dunes/SampleBank.xwb`).
fn bank(game: &GameSource, name: &str) -> Vec<Arc<Pcm>> {
    let path = format!("/{}/SampleBank.xwb", name.replace('\\', "/"));
    match game.read(&path).map_err(|e| e.to_string()).and_then(|b| WaveBank::parse(&b).map_err(|e| e.to_string())) {
        Ok(b) => b.waves.into_iter().map(Arc::new).collect(),
        Err(e) => {
            warn!("sound bank {path}: {e}");
            Vec::new()
        }
    }
}

/// A music section (`MUSIC\DUNES1` -> `/Music/Dunes1.wav` on the disc).
/// The retail Sound.xml names `<AREA>2`/`<AREA>3` sections its disc doesn't
/// ship (only the long `<AREA>1` track); like the original, play what's there.
fn music(game: &GameSource, name: &str) -> Option<Arc<Pcm>> {
    let path = format!("/{}.wav", name.replace('\\', "/"));
    if !game.exists(&path) {
        return None;
    }
    match game.read(&path).map_err(|e| e.to_string()).and_then(|b| wav::decode(&b).map_err(|e| e.to_string())) {
        Ok(p) => Some(Arc::new(p)),
        Err(e) => {
            warn!("music {path}: {e}");
            None
        }
    }
}

/// The ground each environment mostly is: the wheel noise of tyres off the
/// track's own ground (on props).
fn main_surface(env: &str) -> &'static str {
    match env {
        "Dunes" => "Sand",
        "Glacier" => "Snow",
        "Swamp" => "Mud",
        "Stadium" => "Dirt",
        _ => "Dirt",
    }
}

#[derive(Component)]
struct EngineLoop {
    on: bool,
}

/// A computer rider's (or another player's) engine loop: racer index.
#[derive(Component)]
struct OtherEngine {
    racer: usize,
    on: bool,
}

/// How loud another quad is at `d` metres from the nearest player (the
/// original's falloff isn't decoded; this halves by about 3.3 m and is
/// faint past 30).
fn falloff(d: f32) -> f32 {
    (8.0 / (d.max(0.0) + 8.0)).powi(2)
}

/// Where the race is heard from: each player's quad, player 1 first, or
/// the quad each of the attract demo's `views` is on (the original listens
/// from each view's camera; the rewrite's mix has always been relative to
/// the player's quad).
fn listeners(player: &Player, opponents: Option<&crate::opponents::Opponents>, views: usize) -> Vec<Vec3> {
    let others = opponents.into_iter().flat_map(|o| o.racers.iter().enumerate().filter(|(i, r)| r.human.is_some() || i + 1 < views).map(|(_, r)| r.vehicle.position));
    std::iter::once(player.0.position).chain(others).collect()
}

/// How loud a sound at `at` is: by its distance to the nearest player, as
/// the original places it against the nearest view's listener (0x75e50).
fn nearness(listeners: &[Vec3], at: Vec3) -> f32 {
    listeners.iter().map(|l| falloff(l.distance(at))).fold(0.0, f32::max)
}

/// How many of a quad's wheels (from wheel 0) make its wheel noise: with
/// three or four players the original gives each player's quad a voice for
/// wheel 0 only (0x94ef0's mode 1), and with one or two a voice per wheel.
/// Computer riders' wheels are all heard here (the original has none).
fn heard_wheels(players: usize, human: bool) -> usize {
    if players > 2 && human { 1 } else { usize::MAX }
}

/// Wheel noise: player 1's quad, or another racer's (index) by distance.
#[derive(Component, Clone)]
struct WheelLoop {
    volume: f32,
    pitch: f32,
    racer: Option<usize>,
    /// The water loop (its deep-water pitch), heard only while wading.
    water: Option<f32>,
    /// A ground loop: volume and pitch of this sample on each surface (by
    /// collision id; zero volume where another sample plays), and the
    /// surface wheels off the track's own ground count as.
    ground: Option<(Arc<[(f32, f32)]>, u8)>,
}

enum Role {
    Engine(bool),
    Other(usize, bool),
    Wheel(WheelLoop),
    Bed(Bed),
}

/// Music or ambience: its mix level before the volume bar, so it follows
/// the bar when the pause menu's Audio page changes it.
#[derive(Component, Clone, Copy)]
struct Bed {
    base: f32,
    music: bool,
}

/// Engine and wheel loops stop while the game is paused; music and
/// ambience carry on.
#[derive(Component)]
struct PausesWithGame;

/// Mix levels for a quad: the prototype has one (`mPlayerEngineVol`) for
/// both engine loops and the wheel noise, whose samples are as loud as the
/// engine's; retail has `mEngineVol` for "on", `mEngineIdleVol` for "off",
/// and its wheels go by `mSurfaceVol` alone.
#[derive(Resource)]
struct QuadMix {
    on: f32,
    off: f32,
    wheels: f32,
    pitch_zero: f32,
    pitch_span: f32,
    retail: bool,
}

impl QuadMix {
    fn new(setup: &SoundSetup) -> Self {
        if setup.retail() {
            QuadMix {
                on: setup.platform_level("mEngineVol", 1.0),
                off: setup.platform_level("mEngineIdleVol", 1.0),
                wheels: 1.0,
                pitch_zero: 0.6,
                pitch_span: 1.2,
                retail: true,
            }
        } else {
            let engine = setup.mix_level("mPlayerEngineVol", 0.25);
            QuadMix {
                on: engine,
                off: engine,
                wheels: engine,
                pitch_zero: setup.mix_level("mEnginePitchZero", 0.6),
                pitch_span: setup.mix_level("mEnginePitchSpan", 1.2),
                retail: false,
            }
        }
    }

    /// A quad's engine loops, "on" then "off": (volume, pitch).
    fn engine(&self, v: &atv_sim::Vehicle) -> [(f32, f32); 2] {
        let r = (v.revs / v.params.max_revs.max(1.0)).clamp(0.0, 1.0);
        let [on, off] = if self.retail {
            retail_engine_mix(r, v.throttle_pedal(), v.clutch(), v.wheelspin().clamp(0.0, 1.0), v.first_gear())
        } else {
            engine_mix(r, v.throttle, self.pitch_zero, self.pitch_span)
        };
        [(on.0 * self.on, on.1), (off.0 * self.off, off.1)]
    }
}

fn start_race_audio(
    mut commands: Commands,
    game: Res<Game>,
    sel: Res<Selection>,
    settings: Res<Settings>,
    data: Option<Res<Banks>>,
    opponents: Option<Res<crate::opponents::Opponents>>,
    mut sounds: ResMut<Assets<Sound>>,
    (jukebox, demo, mut songs): (Option<ResMut<Jukebox>>, Option<Res<crate::demo::Demo>>, ResMut<Assets<Songs>>),
) {
    let Some(data) = data else { return };
    let env = sel.track.trim_end_matches(|c: char| c.is_ascii_digit()).to_string();
    let setup = &data.setup;
    let scoped = DespawnOnExit(AppState::Race);
    // The retail disc's licensed songs, or the area's own track.
    let mut area_music = true;
    if let Some(mut j) = jukebox
        && let Some(list) = j.race(sel.mode, demo.is_some(), settings.playlist)
    {
        area_music = false;
        if !list.is_empty() {
            info!("race music: {}", list.iter().map(|&i| format!("{} - {}", j.songs[i].artist, j.songs[i].title)).collect::<Vec<_>>().join(", "));
            let base = setup.platform_level("mMusicVol", 1.0);
            commands.spawn((
                AudioPlayer(songs.add(Songs { game: game.0.clone(), paths: list.iter().map(|&i| j.songs[i].path()).collect() })),
                PlaybackSettings::ONCE.with_volume(Volume::Linear(base * gain(settings.music))),
                scoped.clone(),
                Bed { base, music: true },
            ));
        }
    }

    let mut play = |sound: Sound, volume: f32, role: Option<Role>| {
        let mut e = commands.spawn((
            AudioPlayer(sounds.add(sound)),
            PlaybackSettings::ONCE.with_volume(Volume::Linear(volume)),
            scoped.clone(),
        ));
        match role {
            Some(Role::Engine(on)) => e.insert((PausesWithGame, EngineLoop { on })),
            Some(Role::Other(racer, on)) => e.insert((PausesWithGame, OtherEngine { racer, on })),
            Some(Role::Wheel(w)) => e.insert((PausesWithGame, w)),
            Some(Role::Bed(b)) => e.insert(b),
            None => &mut e,
        };
    };

    if area_music && let Some(m) = setup.music(&env) {
        let mut parts: Vec<Arc<Pcm>> = Vec::new();
        let intro = m.intro.as_deref().and_then(|n| music(&game.0, n));
        let has_intro = intro.is_some();
        parts.extend(intro);
        parts.extend(m.sections.iter().filter_map(|n| music(&game.0, n)));
        if parts.len() > usize::from(has_intro) {
            let loop_from = usize::from(has_intro);
            let base = setup.mix_level("mMusicVol", 1.0);
            play(Sound { parts: parts.into(), loop_from }, base * gain(settings.music), Some(Role::Bed(Bed { base, music: true })));
        }
    }

    let ambient = if setup.retail() {
        setup.ambient_volume(&env).unwrap_or(0.35) * setup.platform_level("mEnvironmentVol", 1.0)
    } else {
        setup.mix_level("mAmbientVol", 0.3)
    };
    for name in setup.ambient(&env).into_iter().chain(setup.crowd(&env)) {
        if let Some(bg) = bank(&game.0, name).into_iter().next() {
            play(Sound::looped(bg), ambient * gain(settings.effects), Some(Role::Bed(Bed { base: ambient, music: false })));
        }
    }

    let mix = QuadMix::new(setup);
    let (engine_on, engine_off) = if mix.retail { (RETAIL_ENGINE_ON, RETAIL_ENGINE_OFF) } else { (ENGINE_ON, ENGINE_OFF) };
    for (index, on) in [(engine_on, true), (engine_off, false)] {
        if let Some(pcm) = data.vehicles.get(index) {
            play(Sound::looped(pcm.clone()), 0.0, Some(Role::Engine(on)));
            // Everyone else's engines too, by how far off they are.
            for racer in 0..opponents.as_ref().map_or(0, |o| o.racers.len()) {
                play(Sound::looped(pcm.clone()), 0.0, Some(Role::Other(racer, on)));
            }
        }
    }
    // Each quad's wheels on the ground they're on (one loop per sample,
    // mixed from the wheels on surfaces that use it), and in water.
    let names: &[(&str, usize)] = if mix.retail { &RETAIL_VEHICLE_SAMPLES } else { &VEHICLE_SAMPLES };
    let sample = |name: &str| names.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|&(_, i)| i);
    let surface_vol = if mix.retail { setup.platform_level("mSurfaceVol", 1.0) } else { setup.mix_level("mSurfaceVol", 1.0) };
    let names = atv_sim::surface::NAMES;
    let main = names.iter().position(|n| *n == main_surface(&env)).unwrap_or(0) as u8;
    let mut loops: Vec<(usize, WheelLoop)> = Vec::new();
    let mut by_sample: Vec<(usize, Vec<(f32, f32)>)> = Vec::new();
    for (id, name) in names.iter().enumerate() {
        let Some(s) = setup.surface(&env, name).filter(|s| s.volume > 0.0) else { continue };
        let Some(index) = sample(&s.sample) else { continue };
        let k = by_sample.iter().position(|(i, _)| *i == index).unwrap_or_else(|| {
            by_sample.push((index, vec![(0.0, 1.0); names.len()]));
            by_sample.len() - 1
        });
        by_sample[k].1[id] = (s.volume, s.pitch);
    }
    for (index, mix) in by_sample {
        let ground = Some((Arc::from(mix), main));
        loops.push((index, WheelLoop { volume: surface_vol, pitch: 1.0, racer: None, water: None, ground }));
    }
    let deep_pitch = setup.surface(&env, "Deep_Water").map_or(1.0, |s| s.pitch);
    if let Some(s) = setup.surface(&env, "Shallow_Water")
        && let Some(index) = sample(&s.sample)
    {
        let water = WheelLoop { volume: s.volume * surface_vol, pitch: s.pitch, racer: None, water: Some(deep_pitch), ground: None };
        loops.push((index, water));
    }
    for (index, wheel) in loops {
        let Some(pcm) = data.vehicles.get(index) else { continue };
        play(Sound::looped(pcm.clone()), 0.0, Some(Role::Wheel(wheel.clone())));
        for racer in 0..opponents.as_ref().map_or(0, |o| o.racers.len()) {
            play(Sound::looped(pcm.clone()), 0.0, Some(Role::Wheel(WheelLoop { racer: Some(racer), ..wheel.clone() })));
        }
    }
    commands.insert_resource(mix);
}

/// Engine volumes and pitches from the revs, as the original's 0x93c60
/// does with the quad on the ground: r = revs / max revs, t = throttle.
pub fn engine_mix(r: f32, t: f32, pitch_zero: f32, pitch_span: f32) -> [(f32, f32); 2] {
    let on = ((t + r) * 0.7).clamp(0.2, 1.0);
    let off = (1.0 - on).clamp(0.3, 1.0);
    [(on, pitch_zero + pitch_span * r), (off, 0.85 + 1.8 * r)]
}

/// Retail's engine mix (0xb1690), r = revs / max revs and t = the throttle
/// pedal: "on" comes in sooner than the prototype's, and the clutch counts.
/// While it slips (under 0.3: a gear change, or the throttle off at low
/// speed) "on" drops to 0.3, and the pitch eases toward half revs, except
/// in first gear, where "off" swells with the pedal and wheelspin pulls the
/// pitch up toward the limiter.
pub fn retail_engine_mix(r: f32, t: f32, clutch: f32, wheelspin: f32, first: bool) -> [(f32, f32); 2] {
    let mut on = ((t + r) * 0.85).clamp(0.08, 1.0);
    let mut off = 1.0 - on;
    if clutch >= 0.3 {
        off = off.clamp(0.1, 1.0);
    } else {
        on = 0.3;
        let k = t * 0.3 + 0.7;
        off = if first { (off - k) * clutch + k } else { 0.1 };
    }
    let r = if first { r + (1.0 - r) * wheelspin } else { (r - 0.5) * clutch + 0.5 };
    [(on, 0.6 + 1.2 * r), (off, 0.85 + 1.8 * r)]
}

#[allow(clippy::type_complexity)]
fn engine_and_wheels(
    player: Option<Res<Player>>,
    opponents: Option<Res<crate::opponents::Opponents>>,
    mix: Option<Res<QuadMix>>,
    settings: Res<Settings>,
    virt: Res<Time<Virtual>>,
    (water, demo): (Option<Res<crate::water::Water>>, Option<Res<crate::demo::Demo>>),
    mut sinks: Query<(&mut AudioSink, Option<&EngineLoop>, Option<&WheelLoop>, Option<&OtherEngine>), With<PausesWithGame>>,
) {
    let paused = virt.is_paused();
    for (sink, ..) in &sinks {
        match (paused, sink.is_paused()) {
            (true, false) => sink.pause(),
            (false, true) => sink.play(),
            _ => {}
        }
    }
    let (Some(player), Some(mix)) = (player, mix) else { return };
    let ears = listeners(&player, opponents.as_deref(), demo.as_deref().map_or(1, crate::demo::Demo::views));
    let v = &player.0;
    let [on, off] = mix.engine(v);
    for (mut sink, engine, wheel, other) in &mut sinks {
        let (vol, pitch) = match (engine, wheel, other) {
            (Some(e), ..) => if e.on { on } else { off },
            (_, Some(w), _) => {
                let (q, human) = match w.racer {
                    None => (v, true),
                    Some(i) => match opponents.as_ref().and_then(|opp| opp.racers.get(i)) {
                        Some(r) => (&r.vehicle, r.human.is_some()),
                        None => continue,
                    },
                };
                // This sample's share of the wheels on the ground, and its
                // pitch on their surfaces.
                let (mut contact, mut ground_pitch) = (0.0, 0.0);
                if let Some((mix, main)) = &w.ground {
                    for wheel in q.wheels.iter().take(heard_wheels(ears.len(), human)).filter(|w| w.contact) {
                        let id = if wheel.surface == atv_sim::surface::UNKNOWN { *main } else { wheel.surface };
                        let (vol, pitch) = mix.get(usize::from(id)).copied().unwrap_or((0.0, 1.0));
                        contact += vol;
                        ground_pitch += vol * pitch;
                    }
                }
                let ground_pitch = if contact > 0.0 { ground_pitch / contact } else { 1.0 };
                let contact = contact / q.wheels.len().max(1) as f32;
                let pace = (q.velocity.length() / 20.0).clamp(0.0, 1.0);
                let near = nearness(&ears, q.position);
                let depth = crate::water::depth(water.as_deref(), q.position);
                let (vol, pitch) = match w.water {
                    // Splashing: churns even when slow, lower when deep.
                    Some(deep) if depth > 0.05 => (pace.max(0.3), if depth > crate::water::DEEP { deep } else { w.pitch }),
                    Some(_) => (0.0, w.pitch),
                    None if depth > 0.05 => (0.0, w.pitch),
                    None => (pace * contact, ground_pitch),
                };
                (w.volume * mix.wheels * vol * near, pitch * (0.8 + 0.4 * pace))
            }
            (.., Some(o)) => {
                let Some(q) = opponents.as_ref().and_then(|opp| opp.racers.get(o.racer)).map(|r| &r.vehicle) else { continue };
                let [on, off] = mix.engine(q);
                let (vol, pitch) = if o.on { on } else { off };
                (vol * nearness(&ears, q.position), pitch)
            }
            _ => continue,
        };
        sink.set_volume(Volume::Linear(vol * gain(settings.effects)));
        sink.set_speed(pitch);
    }
}

/// Plays a clip once.
/// Music and ambience follow the volume bars as they change.
fn bed_volumes(settings: Res<Settings>, mut sinks: Query<(&Bed, &mut AudioSink)>) {
    if !settings.is_changed() {
        return;
    }
    for (b, mut sink) in &mut sinks {
        sink.set_volume(Volume::Linear(b.base * gain(if b.music { settings.music } else { settings.effects })));
    }
}

fn one_shot(commands: &mut Commands, sounds: &mut Assets<Sound>, pcm: &Arc<Pcm>, volume: f32, pitch: f32) {
    let sound = Sound { parts: Arc::new([pcm.clone()]), loop_from: 1 };
    commands.spawn((AudioPlayer(sounds.add(sound)), PlaybackSettings::DESPAWN.with_volume(Volume::Linear(volume)).with_speed(pitch)));
}

/// SpotFX in the menus: a tick for the cursor, a clunk to select or go
/// back.
fn menu_sounds(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&bevy::input::gamepad::Gamepad>,
    banks: Option<Res<Banks>>,
    settings: Res<Settings>,
    mut sounds: ResMut<Assets<Sound>>,
    fe: Option<Res<crate::frontend::FrontEnd>>,
) {
    let (Some(banks), Some(nav)) = (banks, crate::ui::read_nav(&keys, &pads)) else { return };
    // Y only does something where a page offers the Trick Book.
    if nav == crate::ui::Nav::Extra && !fe.is_some_and(|fe| crate::frontend::offers_book(fe.screen)) {
        return;
    }
    spot_fx(&mut commands, &mut sounds, &banks, &settings, nav_sound(nav));
}

/// The same cursor and select sounds on the pause and results menus, and
/// the pop-up panel sound as the pause menu opens and closes.
#[allow(clippy::too_many_arguments)]
fn pause_menu_sounds(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&bevy::input::gamepad::Gamepad>,
    banks: Option<Res<Banks>>,
    settings: Res<Settings>,
    mut sounds: ResMut<Assets<Sound>>,
    virt: Res<Time<Virtual>>,
    menus: Query<&crate::hud::RaceMenu>,
    mut was_paused: Local<bool>,
) {
    use crate::ui::Nav;
    let paused = virt.is_paused();
    let toggled = paused != *was_paused;
    *was_paused = paused;
    let Some(banks) = banks else { return };
    let index = match crate::ui::read_nav(&keys, &pads) {
        _ if toggled => 2,
        // On the Audio, Camera and Controller pages they go back a page.
        Some(Nav::Start | Nav::Back) if menus.iter().any(|m| m.page.is_some()) => 1,
        // Start and Back close the pause menu: the panel sound covers it.
        // Y does nothing there.
        Some(Nav::Start | Nav::Back | Nav::Extra) => return,
        Some(nav) if !menus.is_empty() => nav_sound(nav),
        _ => return,
    };
    spot_fx(&mut commands, &mut sounds, &banks, &settings, index);
}

/// SpotFX 0 for cursor moves, 1 for select and back.
fn nav_sound(nav: crate::ui::Nav) -> usize {
    use crate::ui::Nav;
    match nav {
        Nav::Up | Nav::Down | Nav::Left | Nav::Right => 0,
        Nav::Accept | Nav::Start | Nav::Back | Nav::Extra => 1,
    }
}

fn spot_fx(commands: &mut Commands, sounds: &mut Assets<Sound>, banks: &Banks, settings: &Settings, index: usize) {
    if let Some(pcm) = banks.spotfx.get(index) {
        debug!("spotfx {index}");
        let volume = banks.setup.mix_level("mFrontEndSpotFXVol", 0.4) * gain(settings.effects);
        one_shot(commands, sounds, pcm, volume, 1.0);
    }
}

/// Riders-bank clips by voice (0x94690's tables).
const FEMALE_VOICES: std::ops::Range<usize> = 0..14;
const MALE_VOICES: std::ops::Range<usize> = 14..33;
/// Vehicles-bank knocks, softest first (0x94480's table).
const KNOCKS: [usize; 6] = [25, 29, 30, 17, 18, 23];
/// The original's knock threshold (0.3) is in its own impact units; this
/// is the chassis closing speed (m/s) under which a scrape stays quiet.
const KNOCK_MIN: f32 = 2.0;
/// Retail's banks (tables at 0x1ce554, 0x1ce5b8, 0x1ce5e4, 0x1ce6a0): a
/// thrown rider's body thuds, then sometimes cries out, by voice, and
/// the quad's knocks, softest first.
const RETAIL_MALE_VOICES: std::ops::Range<usize> = 12..23;
const RETAIL_FEMALE_VOICES: std::ops::Range<usize> = 37..48;
const RETAIL_THUDS: std::ops::Range<usize> = 48..56;
const RETAIL_KNOCKS: [usize; 5] = [14, 20, 12, 13, 18];

#[derive(Default)]
struct CrashVoices {
    /// Per quad (player 1 first, then the computer riders): seconds since
    /// its last grunt and knock, and how hard each was.
    timers: Vec<[(f32, f32); 2]>,
    /// Per quad: retail's cries since the rider came off (two at most).
    cries: Vec<u8>,
    last_voice: Option<usize>,
    last_knock: Option<usize>,
    seed: u32,
}

impl CrashVoices {
    fn rand(&mut self, n: usize) -> usize {
        self.seed = self.seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        (self.seed >> 16) as usize % n.max(1)
    }

    /// The grunt and knock, if any, from quad `slot`'s crash state. `near`
    /// scales them by how far it is from the nearest player.
    fn play(&mut self, slot: usize, wreck: &mut crate::crash::Wreck, female: bool, near: f32, out: &mut impl FnMut(&Arc<Pcm>, f32, f32), banks: &Banks) {
        if banks.setup.retail() {
            return self.play_retail(slot, wreck, female, near, out, banks);
        }
        let [mut grunt, mut knock_t] = self.timers[slot];
        // A bailed rider hitting the ground: impulse over 2.5 while moving,
        // at most once a second unless it's twice as hard as the last.
        if let Some((impulse, speed)) = wreck.thump.take()
            && impulse > 2.5
            && speed > 2.2347
            && (grunt.0 > 1.0 || impulse > grunt.1 * 2.0)
        {
            let voices = if female { FEMALE_VOICES } else { MALE_VOICES };
            let pick = self.pick(voices, self.last_voice);
            if let Some(pcm) = banks.riders.get(pick) {
                debug!("rider voice {pick} (impulse {impulse:.1})");
                out(pcm, near, 1.0);
                self.last_voice = Some(pick);
                grunt = (0.0, impulse);
            }
        }
        // The quad's body hitting something: harder knocks draw on more of
        // the table, every 0.25 s at most unless twice as hard.
        let knock = std::mem::take(&mut wreck.knock);
        if knock > KNOCK_MIN && (knock_t.0 > 0.25 || knock > knock_t.1 * 2.0) {
            let (choices, scale) = if knock <= 8.0 { (3, 8.0) } else if knock <= 14.0 { (4, 14.0) } else { (6, 14.0) };
            let i = KNOCKS[self.rand(choices)];
            if let Some(pcm) = banks.vehicles.get(i) {
                let volume = (knock / scale + 0.7).min(1.0) * near;
                let pitch = 1.0 + self.rand(1000) as f32 / 2000.0;
                debug!("knock {i} (impact {knock:.1})");
                out(pcm, volume, pitch);
                knock_t = (0.0, knock);
            }
        }
        self.timers[slot] = [grunt, knock_t];
    }

    /// One of `clips` at random, re-rolling a repeat of `last`.
    fn pick(&mut self, clips: std::ops::Range<usize>, last: Option<usize>) -> usize {
        let mut pick = clips.start + self.rand(clips.len());
        for _ in 0..8 {
            if Some(pick) != last {
                break;
            }
            pick = clips.start + self.rand(clips.len());
        }
        pick
    }

    /// Retail's rules for the same events. A thrown rider's body hitting
    /// something faster than 2.2347 m/s (at most every half second) thuds
    /// louder with speed (0xaeaf0), and 61 times in 100 the rider also cries
    /// out, twice a crash at most (0xae930). A knock (0xb1e90) draws on two
    /// clips when soft (at 0.8), four when harder and all five past 12, at
    /// pitch 0.8 to 0.9, never the same clip twice running.
    fn play_retail(&mut self, slot: usize, wreck: &mut crate::crash::Wreck, female: bool, near: f32, out: &mut impl FnMut(&Arc<Pcm>, f32, f32), banks: &Banks) {
        let [mut thud, mut knock_t] = self.timers[slot];
        self.cries.resize(self.timers.len(), 0);
        if wreck.fall.is_none() {
            self.cries[slot] = 0;
        }
        let level = |key: &str| banks.setup.platform_level(key, 1.0);
        if let Some((impulse, speed)) = wreck.thump.take()
            && impulse > 0.2
            && speed > 2.2347
            && thud.0 > 0.5
        {
            let loud = (speed * 0.223_741_46).min(1.0);
            let pick = self.pick(RETAIL_THUDS, None);
            if let Some(pcm) = banks.riders.get(pick) {
                debug!("rider thud {pick} (speed {speed:.1})");
                out(pcm, level("mRiderCollVol") * loud * near, 1.0);
            }
            thud = (0.0, impulse);
            if self.rand(100) < 61 && loud > 0.5 && self.cries[slot] < 2 {
                let voices = if female { RETAIL_FEMALE_VOICES } else { RETAIL_MALE_VOICES };
                let pick = self.pick(voices, self.last_voice);
                if let Some(pcm) = banks.riders.get(pick) {
                    debug!("rider voice {pick}");
                    out(pcm, level("mRiderVoiceVol") * near, 1.0);
                    self.last_voice = Some(pick);
                    self.cries[slot] += 1;
                }
            }
        }
        let knock = std::mem::take(&mut wreck.knock);
        if knock > KNOCK_MIN && (knock_t.0 > 0.25 || knock > knock_t.1 * 2.0) {
            let (choices, volume) = if knock <= 6.0 { (2, 0.8) } else if knock <= 12.0 { (4, 1.0) } else { (5, 1.0) };
            let pick = self.pick(0..choices, self.last_knock.and_then(|k| RETAIL_KNOCKS.iter().position(|&i| i == k)));
            let i = RETAIL_KNOCKS[pick];
            if let Some(pcm) = banks.vehicles.get(i) {
                let pitch = 0.8 + self.rand(1000) as f32 / 10_000.0;
                debug!("knock {i} (impact {knock:.1})");
                out(pcm, level("mBikeCollVol") * volume * near, pitch);
                self.last_knock = Some(i);
                knock_t = (0.0, knock);
            }
        }
        self.timers[slot] = [thud, knock_t];
    }
}

/// Grunts from thrown riders and knocks from quads' bodies: each player's
/// own at full volume, and everyone else's by how far off they are.
#[allow(clippy::too_many_arguments)]
fn crash_sounds(
    mut commands: Commands,
    time: Res<Time>,
    banks: Option<Res<Banks>>,
    wreck: Option<ResMut<crate::crash::Wreck>>,
    player: Option<Res<Player>>,
    mut opponents: Option<ResMut<crate::opponents::Opponents>>,
    (sel, demo): (Res<Selection>, Option<Res<crate::demo::Demo>>),
    settings: Res<Settings>,
    mut sounds: ResMut<Assets<Sound>>,
    mut state: Local<CrashVoices>,
) {
    let (Some(banks), Some(mut wreck), Some(player)) = (banks, wreck, player) else { return };
    let dt = time.delta_secs();
    let quads = 1 + opponents.as_ref().map_or(0, |o| o.racers.len());
    state.timers.resize(quads, [(f32::MAX, 0.0); 2]);
    for t in &mut state.timers {
        t[0].0 += dt;
        t[1].0 += dt;
    }
    let effects = gain(settings.effects);
    let mut out = |pcm: &Arc<Pcm>, volume: f32, pitch: f32| one_shot(&mut commands, &mut sounds, pcm, volume * effects, pitch);
    let female = |model: &str| model.to_ascii_lowercase().starts_with("female");
    state.play(0, &mut wreck, female(&sel.rider), 1.0, &mut out, &banks);
    let ears = listeners(&player, opponents.as_deref(), demo.as_deref().map_or(1, crate::demo::Demo::views));
    if let Some(opp) = opponents.as_mut() {
        for (i, r) in opp.racers.iter_mut().enumerate() {
            let near = nearness(&ears, r.vehicle.position);
            state.play(i + 1, &mut r.wreck, female(&r.model), near, &mut out, &banks);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm(samples: Vec<i16>) -> Arc<Pcm> {
        Arc::new(Pcm { channels: 1, rate: 8000, samples })
    }

    #[test]
    fn intro_then_loop() {
        let s = Sound { parts: Arc::new([pcm(vec![1]), pcm(vec![2, 3]), pcm(vec![4])]), loop_from: 1 };
        let got: Vec<i32> = s.decoder().take(8).map(|x| (x * 32768.0).round() as i32).collect();
        assert_eq!(got, [1, 2, 3, 4, 2, 3, 4, 2]);
    }

    #[test]
    fn empty_sound_ends() {
        let s = Sound { parts: Arc::new([pcm(vec![])]), loop_from: 0 };
        assert_eq!(s.decoder().next(), None);
    }

    #[test]
    fn other_engines_fade_with_distance() {
        assert_eq!(falloff(0.0), 1.0);
        assert!(falloff(8.0) == 0.25 && falloff(40.0) < 0.03);
    }

    #[test]
    fn nearest_player_hears_it() {
        let ears = [Vec3::ZERO, Vec3::new(100.0, 0.0, 0.0)];
        // Player 2's own quad is as loud as player 1's.
        assert_eq!(nearness(&ears, ears[1]), 1.0);
        assert_eq!(nearness(&ears, Vec3::new(92.0, 0.0, 0.0)), falloff(8.0));
        assert_eq!(heard_wheels(2, true), usize::MAX);
        assert_eq!((heard_wheels(3, true), heard_wheels(4, false)), (1, usize::MAX));
    }

    #[test]
    fn engine_idle_and_flat_out() {
        let [on, off] = engine_mix(0.0, 0.0, 0.6, 1.2);
        assert_eq!((on.0, off.0), (0.2, 0.8));
        assert!((on.1 - 0.6).abs() < 1e-6 && (off.1 - 0.85).abs() < 1e-6);
        let [on, off] = engine_mix(1.0, 1.0, 0.6, 1.2);
        assert_eq!((on.0, off.0), (1.0, 0.3));
    }

    #[test]
    fn retail_engine() {
        // Driving in gear: "on" from the pedal and revs, "off" the rest.
        let [on, off] = retail_engine_mix(0.5, 0.5, 1.0, 0.0, false);
        assert!((on.0 - 0.85).abs() < 1e-6 && (off.0 - 0.15).abs() < 1e-6);
        assert!((on.1 - 1.2).abs() < 1e-6 && (off.1 - 1.75).abs() < 1e-6);
        // Idling in first, clutch out: quiet "on", the idle loop at 0.7.
        let [on, off] = retail_engine_mix(0.15, 0.0, 0.0, 0.0, true);
        assert!((on.0 - 0.3).abs() < 1e-6 && (off.0 - 0.7).abs() < 1e-6);
        // Mid-shift the pitch sits at half revs.
        let [on, off] = retail_engine_mix(0.9, 1.0, 0.0, 0.0, false);
        assert!((on.0 - 0.3).abs() < 1e-6 && (off.0 - 0.1).abs() < 1e-6 && (on.1 - 1.2).abs() < 1e-6);
        // Wheelspin in first revs it toward the limiter.
        let [on, _] = retail_engine_mix(0.5, 1.0, 1.0, 1.0, true);
        assert!((on.1 - 1.8).abs() < 1e-6);
    }

    fn jukebox(na: bool) -> Jukebox {
        let song = |n| Song { artist: String::new(), title: String::new(), file: format!("Licensed\\Track{n}"), level_specific: false };
        Jukebox::new((1..=8).map(song).collect(), na, 7)
    }

    #[test]
    fn playlist_in_order_moves_on_each_race() {
        let mut j = jukebox(false);
        assert_eq!(j.race(Mode::SingleRace, false, Playlist::InOrder), Some(vec![0, 1, 2, 3, 4, 5, 6, 7]));
        assert_eq!(j.race(Mode::Career, false, Playlist::InOrder), Some(vec![1, 2, 3, 4, 5, 6, 7, 0]));
        // North America: Track5 is left out and the order skips it.
        let mut j = jukebox(true);
        assert_eq!(j.race(Mode::SingleRace, false, Playlist::InOrder), Some(vec![0, 1, 2, 3, 5, 6, 7]));
        assert_eq!(j.race(Mode::SingleRace, false, Playlist::InOrder).unwrap()[0], 1);
    }

    #[test]
    fn shuffled_plays_each_song_once() {
        let mut j = jukebox(true);
        let mut list = j.race(Mode::Arcade, false, Playlist::Random).unwrap();
        list.sort();
        assert_eq!(list, [0, 1, 2, 3, 5, 6, 7]);
    }

    #[test]
    fn which_races_get_songs() {
        let mut j = jukebox(false);
        assert_eq!(j.race(Mode::SingleRace, false, Playlist::Environment), None);
        assert_eq!(j.race(Mode::SingleRace, false, Playlist::Mute), Some(vec![]));
        assert_eq!(j.race(Mode::Challenge, false, Playlist::Random), Some(vec![]));
        assert_eq!(j.race(Mode::SingleRace, true, Playlist::Random), Some(vec![]));
        // Training: one song at a time, a different one until all have played.
        let mut j = jukebox(true);
        let mut heard: Vec<usize> = (0..7).flat_map(|_| j.race(Mode::Training, false, Playlist::Mute).unwrap()).collect();
        heard.sort();
        assert_eq!(heard, [0, 1, 2, 3, 5, 6, 7]);
        assert_eq!(j.race(Mode::Training, false, Playlist::Mute).unwrap().len(), 1);
    }

    #[test]
    fn playlist_choice_keys() {
        for p in Playlist::ALL {
            assert_eq!(Playlist::parse(p.key()), Some(p));
        }
    }
}
