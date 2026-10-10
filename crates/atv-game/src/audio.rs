//! Sound: each environment's music and ambience, and the player's engine
//! and wheel noise, all decoded from the player's own copy of the game.
//!
//! `Setup/Sound.xml` gives the mix levels, the music sections and the
//! ambient banks. The engine follows the original's update (0x93c60): two
//! looping Vehicles-bank samples, "on" (2) and "off" (1), whose volume
//! and pitch track the revs and throttle.

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
            .add_systems(OnEnter(AppState::Race), start_race_audio)
            .add_systems(Update, engine_and_wheels.run_if(in_state(AppState::Race)));
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

/// Sound.xml and the Vehicles bank, read once.
struct SoundData {
    setup: SoundSetup,
    vehicles: Vec<Arc<Pcm>>,
}

impl SoundData {
    fn load(game: &GameSource) -> Self {
        let setup = game
            .read("/Setup/Sound.xml")
            .map(|b| SoundSetup::parse(&String::from_utf8_lossy(&b)))
            .unwrap_or_else(|e| {
                warn!("no Sound.xml: {e}");
                SoundSetup::default()
            });
        let vehicles = bank(game, "SOUND\\VEHICLES");
        SoundData { setup, vehicles }
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
fn music(game: &GameSource, name: &str) -> Option<Arc<Pcm>> {
    let path = format!("/{}.wav", name.replace('\\', "/"));
    match game.read(&path).map_err(|e| e.to_string()).and_then(|b| wav::decode(&b).map_err(|e| e.to_string())) {
        Ok(p) => Some(Arc::new(p)),
        Err(e) => {
            warn!("music {path}: {e}");
            None
        }
    }
}

/// The ground each environment mostly is, for the wheel noise (the track
/// collision doesn't carry surface types yet).
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

#[derive(Component)]
struct WheelLoop {
    volume: f32,
    pitch: f32,
}

enum Role {
    Engine(bool),
    Wheel(WheelLoop),
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
    mut data: Local<Option<SoundData>>,
    mut sounds: ResMut<Assets<Sound>>,
) {
    let data = data.get_or_insert_with(|| SoundData::load(&game.0));
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
            Some(Role::Wheel(w)) => e.insert((PausesWithGame, w)),
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
            play(Sound { parts: parts.into(), loop_from }, setup.mix_level("mMusicVol", 1.0) * gain(settings.music), None);
        }
    }

    let ambient_vol = setup.mix_level("mAmbientVol", 0.3) * gain(settings.effects);
    for name in setup.ambient(&env).into_iter().chain(setup.crowd(&env)) {
        if let Some(bg) = bank(&game.0, name).into_iter().next() {
            play(Sound::looped(bg), ambient_vol, None);
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
        }
    }
    if let Some(s) = setup.surface(&env, main_surface(&env)) {
        let index = VEHICLE_SAMPLES.iter().find(|(n, _)| n.eq_ignore_ascii_case(&s.sample)).map(|&(_, i)| i);
        if let Some(pcm) = index.and_then(|i| data.vehicles.get(i)) {
            let volume = s.volume * setup.mix_level("mSurfaceVol", 1.0);
            let wheel = WheelLoop { volume, pitch: s.pitch };
            play(Sound::looped(pcm.clone()), 0.0, Some(Role::Wheel(wheel)));
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
    mix: Option<Res<QuadMix>>,
    settings: Res<Settings>,
    virt: Res<Time<Virtual>>,
    mut sinks: Query<(&mut AudioSink, Option<&EngineLoop>, Option<&WheelLoop>), With<PausesWithGame>>,
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
    let v = &player.0;
    let r = (v.revs / v.params.max_revs.max(1.0)).clamp(0.0, 1.0);
    let [on, off] = engine_mix(r, v.throttle, mix.pitch_zero, mix.pitch_span);
    let contact = v.wheels.iter().filter(|w| w.contact).count() as f32 / v.wheels.len().max(1) as f32;
    let pace = (v.velocity.length() / 20.0).clamp(0.0, 1.0);
    for (mut sink, engine, wheel) in &mut sinks {
        let (vol, pitch) = match (engine, wheel) {
            (Some(e), _) => {
                let (vol, pitch) = if e.on { on } else { off };
                (vol * mix.engine, pitch)
            }
            (_, Some(w)) => (w.volume * mix.engine * pace * contact, w.pitch * (0.8 + 0.4 * pace)),
            _ => continue,
        };
        sink.set_volume(Volume::Linear(vol * gain(settings.effects)));
        sink.set_speed(pitch);
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
    fn engine_idle_and_flat_out() {
        let [on, off] = engine_mix(0.0, 0.0, 0.6, 1.2);
        assert_eq!((on.0, off.0), (0.2, 0.8));
        assert!((on.1 - 0.6).abs() < 1e-6 && (off.1 - 0.85).abs() < 1e-6);
        let [on, off] = engine_mix(1.0, 1.0, 0.6, 1.2);
        assert_eq!((on.0, off.0), (1.0, 0.3));
    }
}
