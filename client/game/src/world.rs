//! The static island: ground, water, roads, plots, landmarks and trees.
//! Spawned once at startup so menus can show it as a backdrop.

use std::f32::consts::{FRAC_PI_2, TAU};

use bevy::light::GlobalAmbientLight;
use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;
use bevy_rapier3d::prelude::*;

use crate::art::{Build, pal};
use crate::state::GameData;

#[derive(Component)]
pub struct MainCamera;

/// Tiny deterministic RNG so every client builds the same decoration.
pub struct Lcg(pub u64);
impl Lcg {
    pub fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) as f32) / (1u64 << 31) as f32
    }
    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.next()
    }
}

pub fn setup_camera(mut commands: Commands, mut egui: ResMut<bevy_egui::EguiGlobalSettings>) {
    commands.insert_resource(ClearColor(crate::state::rgb(0x9CC9E8)));
    commands.insert_resource(GlobalAmbientLight { color: Color::WHITE, brightness: 600.0, ..default() });
    let main = commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection { fov: 70f32.to_radians(), far: 3000.0, ..default() }),
        Transform::from_xyz(0.0, 180.0, 420.0).looking_at(Vec3::ZERO, Vec3::Y),
        DistanceFog {
            color: crate::state::rgb(0xB8D6EA),
            directional_light_color: Color::srgba(1.0, 0.95, 0.85, 0.4),
            directional_light_exponent: 30.0,
            falloff: FogFalloff::Linear { start: 350.0, end: 1400.0 },
        },
        MainCamera,
    )).id();
    crate::viewmodel::spawn_camera(&mut commands, main, &mut egui);
    commands.spawn((
        DirectionalLight { illuminance: 9000.0, shadow_maps_enabled: true, ..default() },
        bevy::camera::visibility::RenderLayers::from_layers(&[0, crate::guns::VM_LAYER]),
        Transform::from_xyz(200.0, 400.0, 120.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

/// Adds an extra box collider (optionally rotated) to an existing fixed body.
fn add_box(b: &mut Build, parent: Entity, center: Vec3, he: Vec3, rot: Quat) {
    let e = b
        .commands
        .spawn((Transform { translation: center, rotation: rot, scale: Vec3::ONE }, Visibility::default(), Collider::cuboid(he.x, he.y, he.z)))
        .id();
    b.commands.entity(parent).add_child(e);
}

fn fixed_box(b: &mut Build, center: Vec3, he: Vec3) -> Entity {
    b.commands
        .spawn((Transform::from_translation(center), Visibility::default(), RigidBody::Fixed, Collider::cuboid(he.x, he.y, he.z)))
        .id()
}

pub fn spawn_world(mut b: Build, data: Res<GameData>) {
    let map = &data.map;
    let v = Vec3::new;

    // Ground collider (top at y = 0) plus visuals.
    let ground = b
        .commands
        .spawn((Transform::from_xyz(0.0, -1.0, 0.0), Visibility::default(), RigidBody::Fixed, Collider::cylinder(1.0, map.half_size)))
        .id();
    let sand_r = map.half_size;
    b.cyl(ground, pal::SAND, v(0.0, 0.88, 0.0), sand_r + 1.0, 0.2, Quat::IDENTITY);
    b.cyl(ground, pal::GRASS, v(0.0, 0.92, 0.0), sand_r - 16.0, 0.2, Quat::IDENTITY);
    let water = b.commands.spawn((Transform::from_xyz(0.0, -1.2, 0.0), Visibility::default())).id();
    let wm = b.mat_kind(pal::WATER, 3);
    b.boxm(water, wm, Vec3::ZERO, v(6000.0, 0.1, 6000.0));

    // Dirt: central plaza and a road from every plot to it.
    let dirt = b.commands.spawn((Transform::default(), Visibility::default())).id();
    b.cyl(dirt, pal::DIRT, v(0.0, 0.03, 0.0), 48.0, 0.02, Quat::IDENTITY);
    b.cyl(dirt, pal::GRASS, v(0.0, 0.035, 0.0), 30.0, 0.02, Quat::IDENTITY);
    for i in 0..map.plots.count {
        let (c, yaw) = map.plot(i);
        let len = c.length() - map.plots.size / 2.0 - 40.0;
        let mid = c.normalize() * (40.0 + len / 2.0);
        b.boxr(dirt, pal::DIRT, v(mid.x, 0.025, mid.z), v(7.0, 0.02, len), Quat::from_rotation_y(yaw));
    }
    let ring = 150.0;
    let segs = 32;
    for i in 0..segs {
        let a = (i as f32 + 0.5) * TAU / segs as f32;
        let p = v(a.sin() * ring, 0.028, a.cos() * ring);
        b.boxr(dirt, pal::DIRT, p, v(6.0, 0.02, ring * TAU / segs as f32 + 1.0), Quat::from_rotation_y(a + FRAC_PI_2));
    }

    // Runway.
    let rw = &map.runway;
    let (mn, mx) = (Vec3::from(rw.min), Vec3::from(rw.max));
    b.boxp(dirt, 0x4A4A48, v((mn.x + mx.x) / 2.0, 0.04, (mn.z + mx.z) / 2.0), v(mx.x - mn.x, 0.03, mx.z - mn.z));
    let mut z = mn.z + 6.0;
    while z < mx.z - 6.0 {
        b.boxp(dirt, 0xEEEEEE, v((mn.x + mx.x) / 2.0, 0.06, z), v(0.6, 0.02, 5.0));
        z += 12.0;
    }

    // Plots: concrete pads with hazard edges and amber corner lights.
    for i in 0..map.plots.count {
        let (c, yaw) = map.plot(i);
        let s = map.plots.size;
        let pad = b.commands.spawn((Transform::from_translation(c).with_rotation(Quat::from_rotation_y(yaw)), Visibility::default())).id();
        b.boxp(pad, pal::CONCRETE, v(0.0, 0.04, 0.0), v(s, 0.08, s));
        for k in 0..8 {
            let o = -s / 2.0 + s / 8.0 * k as f32;
            b.boxp(pad, pal::CONCRETE_DARK, v(o, 0.085, 0.0), v(0.08, 0.01, s));
            b.boxp(pad, pal::CONCRETE_DARK, v(0.0, 0.085, o), v(s, 0.01, 0.08));
        }
        for (x, z) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
            b.boxp(pad, pal::CONCRETE_DARK, v(x * s / 2.0, 0.5, z * s / 2.0), v(1.6, 1.0, 1.6));
            add_box(&mut b, pad, v(x * s / 2.0, 0.5, z * s / 2.0), v(0.8, 0.5, 0.8), Quat::IDENTITY);
            let g = b.glow(pal::AMBER);
            b.boxm(pad, g, v(x * s / 2.0, 1.05, z * s / 2.0), v(0.5, 0.1, 0.5));
        }
        b.boxp(pad, pal::HAZARD, v(0.0, 0.09, s / 2.0 - 0.3), v(12.0, 0.02, 0.6));
    }

    // Landmarks from the shared map (their colliders match the server's AABBs).
    for s in &map.statics {
        let (c, he) = (s.center(), s.half_extents());
        let e = fixed_box(&mut b, c, he);
        let n = s.name.as_str();
        if n == "radio_tower" {
            for (x, z) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                b.boxp(e, 0x6F6F6F, v(x * (he.x - 0.2), 0.0, z * (he.z - 0.2)), v(0.35, he.y * 2.0, 0.35));
            }
            let mut y = -he.y + 2.0;
            while y < he.y {
                b.boxp(e, 0x6F6F6F, v(0.0, y, he.z - 0.2), v(he.x * 2.0, 0.2, 0.2));
                b.boxp(e, 0x6F6F6F, v(0.0, y, -he.z + 0.2), v(he.x * 2.0, 0.2, 0.2));
                b.boxp(e, 0x6F6F6F, v(he.x - 0.2, y, 0.0), v(0.2, 0.2, he.z * 2.0));
                b.boxp(e, 0x6F6F6F, v(-he.x + 0.2, y, 0.0), v(0.2, 0.2, he.z * 2.0));
                y += 3.0;
            }
            b.cyl(e, 0xDDDDDD, v(1.5, he.y - 3.0, 1.0), 1.2, 0.3, Quat::from_rotation_x(1.2));
            let g = b.glow(0xFF3020);
            b.boxm(e, g, v(0.0, he.y + 0.4, 0.0), v(0.5, 0.5, 0.5));
        } else if n.contains("rock") {
            b.boxp(e, pal::ROCK, Vec3::ZERO, he * 2.0);
            b.boxr(e, 0x9A958F, v(he.x * 0.2, he.y * 0.6, -he.z * 0.1), he * v(1.2, 0.8, 1.1), Quat::from_rotation_y(0.4));
            add_box(&mut b, e, v(he.x * 0.2, he.y * 0.6, -he.z * 0.1), he * v(1.2, 0.8, 1.1) / 2.0, Quat::from_rotation_y(0.4));
        } else if n.starts_with("town") || n == "forest_cabin" {
            let wall = if n == "forest_cabin" { pal::WOOD } else { 0xD9CBA8 };
            b.boxp(e, wall, Vec3::ZERO, he * 2.0);
            b.boxp(e, pal::ROOF, v(0.0, he.y + 0.4, 0.0), v(he.x * 2.0 + 0.8, 0.8, he.z * 2.0 + 0.8));
            add_box(&mut b, e, v(0.0, he.y + 0.4, 0.0), v(he.x + 0.4, 0.4, he.z + 0.4), Quat::IDENTITY);
            let mut x = -he.x + 2.0;
            while x < he.x - 1.0 {
                b.boxp(e, 0x3A4A5A, v(x, he.y * 0.3, he.z + 0.01), v(1.0, 1.2, 0.05));
                x += 3.0;
            }
            b.boxp(e, pal::WOOD, v(0.0, -he.y + 1.1, he.z + 0.02), v(1.4, 2.2, 0.05));
        } else if n == "hangar" {
            b.boxp(e, pal::CONCRETE_DARK, Vec3::ZERO, he * 2.0);
            b.boxp(e, 0x50524F, v(0.0, -he.y + 5.0, he.z + 0.02), v(he.x * 1.6, 10.0, 0.05));
            b.boxp(e, pal::HAZARD, v(0.0, -he.y + 10.3, he.z + 0.03), v(he.x * 1.6, 0.5, 0.05));
            add_box(&mut b, e, v(0.0, he.y + 0.1, 0.0), v(he.x + 0.1, 0.1, he.z + 0.1), Quat::IDENTITY);
        } else {
            b.boxp(e, pal::CONCRETE, Vec3::ZERO, he * 2.0);
            b.boxp(e, pal::CONCRETE_DARK, v(0.0, he.y, 0.0), v(he.x * 2.0 + 0.2, 0.2, he.z * 2.0 + 0.2));
            add_box(&mut b, e, v(0.0, he.y, 0.0), v(he.x + 0.1, 0.1, he.z + 0.1), Quat::IDENTITY);
            if n.starts_with("bunker") {
                b.boxp(e, pal::BLACK, v(0.0, he.y * 0.3, he.z + 0.01), v(he.x, 0.5, 0.05));
                b.boxp(e, pal::BLACK, v(0.0, he.y * 0.3, -he.z - 0.01), v(he.x, 0.5, 0.05));
            }
        }
    }

    // Sandbag rings and tank traps around the centre.
    let deco = b.commands.spawn((Transform::default(), Visibility::default())).id();
    let mut rng = Lcg(map.trees.seed ^ 0xABCD);
    for k in 0..14 {
        let a = k as f32 * TAU / 14.0 + 0.2;
        let r = rng.range(40.0, 70.0);
        let p = v(a.sin() * r, 0.0, a.cos() * r);
        let trap = fixed_box(&mut b, p + v(0.0, 0.6, 0.0), v(0.9, 0.6, 0.9));
        b.boxr(trap, 0x555555, Vec3::ZERO, v(2.0, 0.25, 0.25), Quat::from_rotation_z(0.7));
        b.boxr(trap, 0x555555, Vec3::ZERO, v(0.25, 0.25, 2.0), Quat::from_rotation_x(0.7));
        b.boxr(trap, 0x555555, Vec3::ZERO, v(0.25, 2.0, 0.25), Quat::from_rotation_y(0.7));
    }
    for k in 0..10 {
        let a = k as f32 * TAU / 10.0;
        let p = v(a.sin() * 36.0, 0.5, a.cos() * 36.0);
        let bags = fixed_box(&mut b, p, v(3.0, 0.5, 0.5));
        b.commands.entity(bags).insert(Transform::from_translation(p).with_rotation(Quat::from_rotation_y(a + FRAC_PI_2)));
        for j in 0..4 {
            b.boxp(bags, pal::KHAKI, v(-2.25 + j as f32 * 1.5, -0.2, 0.0), v(1.4, 0.5, 0.9));
            if j < 3 {
                b.boxp(bags, 0xA89C66, v(-1.5 + j as f32 * 1.5, 0.3, 0.0), v(1.4, 0.45, 0.85));
            }
        }
    }
    let _ = deco;

    // Coastal boulders, avoiding plot fronts.
    for _ in 0..46 {
        let a = rng.range(0.0, TAU);
        let near_plot = (0..map.plots.count).any(|i| {
            let (c, _) = map.plot(i);
            let pa = c.x.atan2(c.z);
            let d = (a - pa).rem_euclid(TAU);
            d.min(TAU - d) < 0.22
        });
        if near_plot {
            continue;
        }
        let r = rng.range(map.half_size - 40.0, map.half_size - 6.0);
        let he = v(rng.range(2.0, 6.0), rng.range(1.5, 5.0), rng.range(2.0, 6.0));
        let p = v(a.sin() * r, he.y * 0.8, a.cos() * r);
        let e = fixed_box(&mut b, p, he);
        b.commands.entity(e).insert(Transform::from_translation(p).with_rotation(Quat::from_rotation_y(a)));
        b.boxp(e, pal::ROCK, Vec3::ZERO, he * 2.0);
        b.boxr(e, 0x99938C, v(0.0, he.y * 0.7, 0.0), he * v(1.3, 0.6, 1.3), Quat::from_rotation_y(0.6));
        add_box(&mut b, e, v(0.0, he.y * 0.7, 0.0), he * v(1.3, 0.6, 1.3) / 2.0, Quat::from_rotation_y(0.6));
    }

    // Trees: a forest plus scattered pines away from plots and roads.
    let t = &map.trees;
    let mut rng = Lcg(t.seed);
    let mut placed = 0;
    let mut tries = 0;
    while placed < t.count + 80 && tries < 4000 {
        tries += 1;
        let (p, s) = if placed < t.count {
            let a = rng.range(0.0, TAU);
            let r = t.radius * rng.next().sqrt();
            (Vec3::from(t.center) + v(a.sin() * r, 0.0, a.cos() * r), rng.range(0.8, 1.4))
        } else {
            let a = rng.range(0.0, TAU);
            let r = rng.range(60.0, map.half_size - 30.0);
            (v(a.sin() * r, 0.0, a.cos() * r), rng.range(0.7, 1.3))
        };
        if map.plot_at(p).is_some()
            || (0..map.plots.count).any(|i| map.plot(i).0.distance(p) < map.plots.size * 0.75)
            || map.statics.iter().any(|st| st.center().distance(p) < st.half_extents().length() + 3.0)
            || (p.x > -165.0 && p.x < -135.0 && p.z > -75.0 && p.z < 95.0)
            || (p.length() - 150.0).abs() < 6.0
        {
            continue;
        }
        let e = b
            .commands
            .spawn((Transform::from_translation(p), Visibility::default(), RigidBody::Fixed))
            .with_children(|c| {
                c.spawn((Transform::from_xyz(0.0, 1.5 * s, 0.0), Collider::cylinder(1.5 * s, 0.35 * s)));
            })
            .id();
        b.tree(e, s);
        placed += 1;
    }
}

/// Slow orbit of the island behind the menus.
pub fn menu_camera(time: Res<Time>, mut cam: Query<&mut Transform, With<MainCamera>>) {
    let a = time.elapsed_secs() * 0.04;
    for mut t in &mut cam {
        *t = Transform::from_xyz(a.sin() * 430.0, 170.0, a.cos() * 430.0).looking_at(Vec3::new(0.0, 0.0, 0.0), Vec3::Y);
    }
}
