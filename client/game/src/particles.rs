//! Lightweight particles and bullet-hole decals: sparks, dust, smoke, fire and
//! debris. Particles share meshes and materials and fade by shrinking.

use std::collections::VecDeque;

use bevy::light::NotShadowCaster;
use bevy::prelude::*;

use crate::art::{Build, pal};
use crate::state::AppState;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Burst {
    Spark,
    Dust,
    Smoke,
    Fire,
    Debris,
}

#[derive(Message)]
pub struct Emit {
    pub kind: Burst,
    pub pos: Vec3,
    pub normal: Vec3,
    pub count: u32,
    /// Leave a bullet hole at the impact point.
    pub decal: bool,
}

impl Emit {
    pub fn new(kind: Burst, pos: Vec3, normal: Vec3, count: u32) -> Self {
        Self { kind, pos, normal, count, decal: false }
    }
    /// A bullet impact on the world: sparks, dust and a hole.
    pub fn impact(pos: Vec3, normal: Vec3) -> [Emit; 2] {
        [
            Emit { kind: Burst::Spark, pos, normal, count: 5, decal: true },
            Emit { kind: Burst::Dust, pos, normal, count: 2, decal: false },
        ]
    }
}

#[derive(Component)]
pub struct Particle {
    vel: Vec3,
    life: f32,
    max: f32,
    /// Downward acceleration (negative rises).
    grav: f32,
    drag: f32,
    s0: f32,
    s1: f32,
}

#[derive(Component)]
pub struct Decal {
    life: f32,
}

#[derive(Resource, Default)]
pub struct Decals(VecDeque<Entity>);

const MAX_DECALS: usize = 70;

fn rnd() -> f32 {
    rand::random::<f32>()
}

fn unit_in_cone(normal: Vec3, spread: f32) -> Vec3 {
    let random = Vec3::new(rnd() * 2.0 - 1.0, rnd() * 2.0 - 1.0, rnd() * 2.0 - 1.0).normalize_or_zero();
    (normal.normalize_or_zero() + random * spread).normalize_or_zero()
}

pub fn emit(mut events: MessageReader<Emit>, mut b: Build, mut decals: ResMut<Decals>, mut commands_extra: Commands) {
    for ev in events.read() {
        let (mat, cube, speed, life, grav, drag, sizes, spread) = match ev.kind {
            Burst::Spark => (b.glow(pal::AMBER), true, (5.0, 13.0), (0.2, 0.45), 16.0, 0.5, (0.07, 0.01), 0.7),
            Burst::Dust => (b.mat(0xB5AC9C), false, (0.8, 2.6), (0.5, 0.9), -0.6, 1.5, (0.18, 0.7), 0.9),
            Burst::Smoke => (b.mat(0x5A5A5A), false, (0.4, 1.6), (1.2, 2.2), -0.9, 0.8, (0.35, 1.7), 0.8),
            Burst::Fire => (b.glow(pal::HAZARD), false, (2.0, 9.0), (0.3, 0.6), 0.0, 2.0, (0.6, 0.1), 3.0),
            Burst::Debris => (b.mat(pal::ROCK), true, (5.0, 14.0), (0.8, 1.5), 22.0, 0.2, (0.2, 0.12), 1.4),
        };
        let mesh = if cube { b.art.cube.clone() } else { b.art.sphere.clone() };
        for _ in 0..ev.count {
            let dir = unit_in_cone(if ev.kind == Burst::Fire { Vec3::Y * 0.4 } else { ev.normal }, spread);
            let max = life.0 + rnd() * (life.1 - life.0);
            b.commands.spawn((
                Mesh3d(mesh.clone()),
                MeshMaterial3d(mat.clone()),
                Transform::from_translation(ev.pos + ev.normal * 0.05).with_scale(Vec3::splat(sizes.0)),
                Particle { vel: dir * (speed.0 + rnd() * (speed.1 - speed.0)), life: max, max, grav, drag, s0: sizes.0, s1: sizes.1 },
                NotShadowCaster,
                DespawnOnExit(AppState::InGame),
            ));
        }
        if ev.decal && ev.normal.length_squared() > 0.1 {
            let n = ev.normal.normalize();
            let up = if n.y.abs() > 0.95 { Vec3::X } else { Vec3::Y };
            let hole = b.mat(0x151515);
            let mesh = b.art.cube.clone();
            let e = commands_extra
                .spawn((
                    Mesh3d(mesh),
                    MeshMaterial3d(hole),
                    Transform::from_translation(ev.pos + n * 0.012).looking_to(-n, up).with_scale(Vec3::new(0.14, 0.14, 0.01)),
                    Decal { life: 30.0 },
                    NotShadowCaster,
                    DespawnOnExit(AppState::InGame),
                ))
                .id();
            decals.0.push_back(e);
            while decals.0.len() > MAX_DECALS {
                if let Some(old) = decals.0.pop_front() {
                    commands_extra.entity(old).try_despawn();
                }
            }
        }
    }
}

pub fn update(
    time: Res<Time>,
    mut commands: Commands,
    mut parts: Query<(Entity, &mut Particle, &mut Transform), Without<Decal>>,
    mut decals: Query<(Entity, &mut Decal)>,
) {
    let dt = time.delta_secs().min(0.05);
    for (e, mut p, mut t) in &mut parts {
        p.life -= dt;
        if p.life <= 0.0 {
            commands.entity(e).despawn();
            continue;
        }
        p.vel.y -= p.grav * dt;
        let drag = (1.0 - p.drag * dt).max(0.0);
        p.vel *= drag;
        t.translation += p.vel * dt;
        let age = 1.0 - p.life / p.max;
        let size = p.s0 + (p.s1 - p.s0) * age;
        // Shrink out over the last stretch so smoke and dust fade away.
        t.scale = Vec3::splat((size * (1.0 - age.powi(3))).max(0.001));
    }
    for (e, mut d) in &mut decals {
        d.life -= dt;
        if d.life <= 0.0 {
            commands.entity(e).despawn();
        }
    }
}
