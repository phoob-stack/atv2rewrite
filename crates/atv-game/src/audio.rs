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

use std::sync::Arc;

use bevy::audio::{AddAudioSource, ChannelCount, Decodable, SampleRate, Source, Volume};
use bevy::prelude::*;

use atv_formats::sound::SoundSetup;
use atv_formats::wav::{self, Pcm};
use atv_formats::xwb::WaveBank;
use atv_formats::GameSource;

use crate::settings::{gain, Settings};
use crate::{AppState, Game, Player, Selection};

pub struct AudioPlugin;

impl Plugin for AudioPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<Sound>()
            .add_audio_source::<Sound>()
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

/// Mix level for the player's quad (`mPlayerEngineVol`); the wheel noise
/// shares it, as its samples are as loud as the engine's.
#[derive(Resource)]
struct QuadMix {
    engine: f32,
    pitch_zero: f32,
    pitch_span: f32,
}

fn start_race_audio(
    mut commands: Commands,
    game: Res<Game>,
    sel: Res<Selection>,
    settings: Res<Settings>,
    data: Option<Res<Banks>>,
    opponents: Option<Res<crate::opponents::Opponents>>,
    mut sounds: ResMut<Assets<Sound>>,
) {
    let Some(data) = data else { return };
    let env = sel.track.trim_end_matches(|c: char| c.is_ascii_digit()).to_string();
    let setup = &data.setup;
    let scoped = DespawnOnExit(AppState::Race);
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

    if let Some(m) = setup.music(&env) {
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

    let ambient = setup.mix_level("mAmbientVol", 0.3);
    for name in setup.ambient(&env).into_iter().chain(setup.crowd(&env)) {
        if let Some(bg) = bank(&game.0, name).into_iter().next() {
            play(Sound::looped(bg), ambient * gain(settings.effects), Some(Role::Bed(Bed { base: ambient, music: false })));
        }
    }

    let mix = QuadMix {
        engine: setup.mix_level("mPlayerEngineVol", 0.25),
        pitch_zero: setup.mix_level("mEnginePitchZero", 0.6),
        pitch_span: setup.mix_level("mEnginePitchSpan", 1.2),
    };
    for (index, on) in [(ENGINE_ON, true), (ENGINE_OFF, false)] {
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
    let sample = |name: &str| VEHICLE_SAMPLES.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|&(_, i)| i);
    let surface_vol = setup.mix_level("mSurfaceVol", 1.0);
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
    let r = (v.revs / v.params.max_revs.max(1.0)).clamp(0.0, 1.0);
    let [on, off] = engine_mix(r, v.throttle, mix.pitch_zero, mix.pitch_span);
    for (mut sink, engine, wheel, other) in &mut sinks {
        let (vol, pitch) = match (engine, wheel, other) {
            (Some(e), ..) => {
                let (vol, pitch) = if e.on { on } else { off };
                (vol * mix.engine, pitch)
            }
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
                (w.volume * mix.engine * vol * near, pitch * (0.8 + 0.4 * pace))
            }
            (.., Some(o)) => {
                let Some(q) = opponents.as_ref().and_then(|opp| opp.racers.get(o.racer)).map(|r| &r.vehicle) else { continue };
                let r = (q.revs / q.params.max_revs.max(1.0)).clamp(0.0, 1.0);
                let [on, off] = engine_mix(r, q.throttle, mix.pitch_zero, mix.pitch_span);
                let (vol, pitch) = if o.on { on } else { off };
                (vol * mix.engine * nearness(&ears, q.position), pitch)
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

#[derive(Default)]
struct CrashVoices {
    /// Per quad (player 1 first, then the computer riders): seconds since
    /// its last grunt and knock, and how hard each was.
    timers: Vec<[(f32, f32); 2]>,
    last_voice: Option<usize>,
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
        let [mut grunt, mut knock_t] = self.timers[slot];
        // A bailed rider hitting the ground: impulse over 2.5 while moving,
        // at most once a second unless it's twice as hard as the last.
        if let Some((impulse, speed)) = wreck.thump.take()
            && impulse > 2.5
            && speed > 2.2347
            && (grunt.0 > 1.0 || impulse > grunt.1 * 2.0)
        {
            let voices = if female { FEMALE_VOICES } else { MALE_VOICES };
            let mut pick = voices.start + self.rand(voices.len());
            for _ in 0..8 {
                if Some(pick) != self.last_voice {
                    break;
                }
                pick = voices.start + self.rand(voices.len());
            }
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
}
