//! Sound effects, synthesised at startup (no asset files): gunshots per weapon
//! class, reloads, hit ticks, explosions, footsteps, an engine loop and UI clicks.
//! Everything plays through `Play` messages; positional sounds are spatial.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::audio::{AudioSink, AudioSinkPlayback, PlaybackSettings, SpatialListener, Volume};
use bevy::prelude::*;
use bevy_egui::EguiContexts;

use crate::objects::MotionVel;
use crate::player::{LocalPlayer, Seat};
use crate::settings::Settings;
use crate::world::MainCamera;

const RATE: u32 = 22_050;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Snd {
    Pistol,
    Smg,
    Rifle,
    Shotgun,
    Sniper,
    Rocket,
    Swish,
    Reload,
    Hit,
    Head,
    Hurt,
    Kill,
    Boom,
    Step,
    Click,
    Capture,
    Engine,
}

impl Snd {
    /// The gunshot sound for a weapon id.
    pub fn shot(weapon: &str) -> Snd {
        match weapon {
            "pistol" => Snd::Pistol,
            "smg" | "mounted_gun" | "plane_gun" => Snd::Smg,
            "shotgun" => Snd::Shotgun,
            "sniper" => Snd::Sniper,
            "rocket" | "tank_cannon" => Snd::Rocket,
            "knife" | "grenade" => Snd::Swish,
            _ => Snd::Rifle,
        }
    }
}

#[derive(Message)]
pub struct Play {
    pub snd: Snd,
    pub at: Option<Vec3>,
    pub gain: f32,
}

impl Play {
    pub fn ui(snd: Snd) -> Self {
        Self { snd, at: None, gain: 1.0 }
    }
    pub fn at(snd: Snd, pos: Vec3, gain: f32) -> Self {
        Self { snd, at: Some(pos), gain }
    }
}

#[derive(Resource, Default)]
pub struct Bank(HashMap<Snd, Handle<AudioSource>>);

#[derive(Component)]
pub struct EngineSound;

// ---------- synthesis ----------

struct Noise(u32);
impl Noise {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1664525).wrapping_add(1013904223);
        (self.0 >> 8) as f32 / (1u32 << 23) as f32 - 1.0
    }
}

fn wav(samples: &[f32]) -> Arc<[u8]> {
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    let data_len = (samples.len() * 2) as u32;
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 30000.0) as i16).to_le_bytes());
    }
    out.into()
}

fn render(secs: f32, mut f: impl FnMut(f32, usize) -> f32) -> Vec<f32> {
    let n = (secs * RATE as f32) as usize;
    (0..n).map(|i| f(i as f32 / RATE as f32, i)).collect()
}

/// Noise burst through a one-pole low-pass (`lp` closer to 1 = darker) plus a falling thump.
fn blast(secs: f32, decay: f32, lp: f32, thump_hz: f32, thump: f32, crack: f32, seed: u32) -> Vec<f32> {
    let mut n = Noise(seed);
    let mut state = 0.0f32;
    let mut phase = 0.0f32;
    render(secs, |t, _| {
        let white = n.next();
        state = state * lp + white * (1.0 - lp);
        let env = (-t * decay).exp();
        let hz = thump_hz * (1.0 + 2.5 * (-t * 30.0).exp());
        phase += hz / RATE as f32;
        let body = (phase * std::f32::consts::TAU).sin() * thump * (-t * decay * 0.7).exp();
        let snap = white * crack * (-t * 220.0).exp();
        (state * 1.8 * env + body + snap) * 0.7
    })
}

fn tone(secs: f32, hz: f32, decay: f32, vol: f32) -> Vec<f32> {
    render(secs, |t, _| (t * hz * std::f32::consts::TAU).sin() * (-t * decay).exp() * vol)
}

fn mix(parts: &[(f32, Vec<f32>)]) -> Vec<f32> {
    let len = parts.iter().map(|(at, s)| (*at * RATE as f32) as usize + s.len()).max().unwrap_or(0);
    let mut out = vec![0.0; len];
    for (at, s) in parts {
        let o = (*at * RATE as f32) as usize;
        for (i, v) in s.iter().enumerate() {
            out[o + i] += v;
        }
    }
    out
}

fn synth(s: Snd) -> Vec<f32> {
    match s {
        Snd::Pistol => blast(0.22, 22.0, 0.55, 150.0, 0.5, 0.7, 1),
        Snd::Smg => blast(0.14, 34.0, 0.5, 170.0, 0.35, 0.6, 2),
        Snd::Rifle => blast(0.3, 18.0, 0.62, 110.0, 0.7, 0.6, 3),
        Snd::Shotgun => blast(0.5, 11.0, 0.7, 80.0, 0.9, 0.8, 4),
        Snd::Sniper => {
            let mut v = blast(0.9, 7.0, 0.78, 70.0, 0.9, 0.9, 5);
            let echo = blast(0.6, 9.0, 0.88, 60.0, 0.2, 0.0, 6);
            v = mix(&[(0.0, v), (0.11, echo.iter().map(|x| x * 0.35).collect())]);
            v
        }
        Snd::Rocket => blast(1.0, 4.5, 0.9, 55.0, 0.8, 0.2, 7),
        Snd::Swish => {
            let mut n = Noise(8);
            let mut st = 0.0;
            render(0.18, |t, _| {
                st = st * 0.8 + n.next() * 0.2;
                st * (t / 0.18 * std::f32::consts::PI).sin() * 0.6
            })
        }
        Snd::Reload => {
            let click = |seed| blast(0.05, 90.0, 0.3, 300.0, 0.15, 0.9, seed);
            mix(&[(0.0, click(10)), (0.22, click(11).iter().map(|x| x * 1.2).collect()), (0.34, click(12).iter().map(|x| x * 0.6).collect())])
        }
        Snd::Hit => tone(0.07, 1900.0, 55.0, 0.5),
        Snd::Head => mix(&[(0.0, tone(0.2, 2500.0, 18.0, 0.5)), (0.0, tone(0.2, 3700.0, 22.0, 0.3))]),
        Snd::Hurt => blast(0.3, 16.0, 0.9, 90.0, 0.8, 0.3, 13),
        Snd::Kill => mix(&[(0.0, tone(0.22, 880.0, 14.0, 0.45)), (0.09, tone(0.35, 1320.0, 9.0, 0.45))]),
        Snd::Boom => blast(1.8, 2.8, 0.93, 45.0, 1.0, 0.4, 14),
        Snd::Step => blast(0.1, 40.0, 0.86, 90.0, 0.35, 0.1, 15),
        Snd::Click => tone(0.04, 1100.0, 90.0, 0.4),
        Snd::Capture => mix(&[
            (0.0, tone(0.3, 523.0, 7.0, 0.4)),
            (0.12, tone(0.3, 659.0, 7.0, 0.4)),
            (0.24, tone(0.5, 784.0, 5.0, 0.4)),
        ]),
        Snd::Engine => {
            // Exactly one second of whole cycles so the loop is seamless.
            render(1.0, |t, _| {
                let a = (t * 48.0 * std::f32::consts::TAU).sin();
                let b = (t * 96.0 * std::f32::consts::TAU).sin() * 0.5;
                let c = (t * 144.0 * std::f32::consts::TAU).sin() * 0.3;
                let saw = ((t * 48.0).fract() * 2.0 - 1.0) * 0.25;
                (a + b + c + saw) * 0.3
            })
        }
    }
}

pub fn setup_audio(mut commands: Commands, mut sources: ResMut<Assets<AudioSource>>) {
    use Snd::*;
    let mut bank = Bank::default();
    for s in [Pistol, Smg, Rifle, Shotgun, Sniper, Rocket, Swish, Reload, Hit, Head, Hurt, Kill, Boom, Step, Click, Capture, Engine] {
        bank.0.insert(s, sources.add(AudioSource { bytes: wav(&synth(s)) }));
    }
    commands.insert_resource(bank);
}

// ---------- playback ----------

pub fn attach_listener(mut commands: Commands, cam: Query<Entity, (With<MainCamera>, Without<SpatialListener>)>) {
    for e in &cam {
        commands.entity(e).insert(SpatialListener::new(0.25));
    }
}

pub fn apply_volume(settings: Res<Settings>, mut global: ResMut<bevy::audio::GlobalVolume>) {
    if settings.is_changed() {
        global.volume = Volume::Linear(settings.volume);
    }
}

pub fn play_sounds(mut commands: Commands, bank: Res<Bank>, mut events: MessageReader<Play>) {
    for ev in events.read() {
        let Some(h) = bank.0.get(&ev.snd) else { continue };
        let jitter = 0.94 + rand::random::<f32>() * 0.12;
        let mut settings = PlaybackSettings::DESPAWN.with_volume(Volume::Linear(ev.gain)).with_speed(jitter);
        match ev.at {
            Some(pos) => {
                settings = settings.with_spatial(true);
                commands.spawn((AudioPlayer::new(h.clone()), settings, Transform::from_translation(pos)));
            }
            None => {
                commands.spawn((AudioPlayer::new(h.clone()), settings));
            }
        }
    }
}

/// Footsteps while running on foot.
pub fn footsteps(
    time: Res<Time>,
    seat: Res<Seat>,
    me: Query<(&MotionVel, &Transform), With<LocalPlayer>>,
    mut acc: Local<f32>,
    mut play: MessageWriter<Play>,
) {
    let Ok((v, t)) = me.single() else { return };
    let speed = Vec2::new(v.0.x, v.0.z).length();
    if seat.0.is_some() || speed < 1.5 || v.0.y.abs() > 1.0 {
        return;
    }
    *acc += speed * time.delta_secs();
    if *acc > 2.1 {
        *acc = 0.0;
        play.write(Play::at(Snd::Step, t.translation - Vec3::Y * 0.9, 0.5));
    }
}

/// Engine loop whose pitch follows the vehicle's speed while you're seated.
pub fn engine(
    mut commands: Commands,
    bank: Res<Bank>,
    seat: Res<Seat>,
    vel: Query<&bevy_rapier3d::prelude::Velocity>,
    mut sink: Query<(Entity, &mut AudioSink), With<EngineSound>>,
) {
    let Some(s) = seat.0 else {
        for (e, _) in &sink {
            commands.entity(e).despawn();
        }
        return;
    };
    if let Ok((_, mut sk)) = sink.single_mut() {
        let speed = vel.get(s.vehicle).map(|v| v.linear.length()).unwrap_or(0.0);
        sk.set_speed((0.7 + speed * 0.04).clamp(0.7, 2.4));
        sk.set_volume(Volume::Linear((0.18 + speed * 0.01).min(0.5)));
    } else if let Some(h) = bank.0.get(&Snd::Engine) {
        commands.spawn((
            AudioPlayer::new(h.clone()),
            PlaybackSettings::LOOP.with_volume(Volume::Linear(0.2)),
            EngineSound,
        ));
    }
}

/// A click whenever the mouse presses on any egui panel or button.
pub fn ui_clicks(mut contexts: EguiContexts, mut play: MessageWriter<Play>) -> Result {
    let ctx = contexts.ctx_mut()?;
    if ctx.input(|i| i.pointer.primary_pressed()) && ctx.is_pointer_over_egui() {
        play.write(Play::ui(Snd::Click));
    }
    Ok(())
}
