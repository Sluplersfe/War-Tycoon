//! Procedural low-poly models built from unit primitives, following the art
//! references: olive/khaki camo, team accents, grey concrete, amber hazard trim.

use std::collections::HashMap;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

pub mod pal {
    pub const OLIVE: u32 = 0x5E6B3A;
    pub const OLIVE_DARK: u32 = 0x454E2C;
    pub const KHAKI: u32 = 0x9C8F5A;
    pub const SKIN: u32 = 0xE0B48C;
    pub const BOOT: u32 = 0x3B3B32;
    pub const GUNMETAL: u32 = 0x2E3033;
    pub const CONCRETE: u32 = 0xA3A19B;
    pub const CONCRETE_DARK: u32 = 0x75736E;
    pub const HAZARD: u32 = 0xE8A317;
    pub const BLACK: u32 = 0x222222;
    pub const GRASS: u32 = 0x6E8B3D;
    pub const SAND: u32 = 0xD8B97A;
    pub const DIRT: u32 = 0xB08850;
    pub const ROCK: u32 = 0x8A8580;
    pub const WATER: u32 = 0x3C8DC4;
    pub const PINE: u32 = 0x2F5E3A;
    pub const WOOD: u32 = 0x6B4A2B;
    pub const ROOF: u32 = 0xB5402F;
    pub const ORE: u32 = 0x8E9AA6;
    pub const GLASS: u32 = 0x9FC4D8;
    pub const AMBER: u32 = 0xFFB030;
}

#[derive(Resource, Default)]
pub struct Art {
    pub cube: Handle<Mesh>,
    pub cylinder: Handle<Mesh>,
    pub sphere: Handle<Mesh>,
    pub cone: Handle<Mesh>,
    mats: HashMap<(u32, u8), Handle<StandardMaterial>>,
}

pub fn setup_art(mut art: ResMut<Art>, mut meshes: ResMut<Assets<Mesh>>) {
    art.cube = meshes.add(Cuboid::new(1.0, 1.0, 1.0));
    art.cylinder = meshes.add(Cylinder::new(0.5, 1.0));
    art.sphere = meshes.add(Sphere::new(0.5).mesh().ico(2).unwrap());
    art.cone = meshes.add(Cone { radius: 0.5, height: 1.0 });
}

/// Marks a model part that should spin (rotors, propellers).
#[derive(Component)]
pub struct Spinner {
    pub axis: Vec3,
    pub speed: f32,
}

/// Turret part of a vehicle (yaw), with its barrel as a child (pitch).
#[derive(Component)]
pub struct TurretPart;

#[derive(Component)]
pub struct BarrelPart;

/// A swinging limb of a soldier model (pivot at the hip or shoulder).
#[derive(Component)]
pub struct Limb {
    pub leg: bool,
    pub side: f32,
}

/// Walk-cycle state on a soldier root, driven by how fast it actually moves.
#[derive(Component, Default)]
pub struct Gait {
    last: Vec3,
    primed: bool,
    speed: f32,
    amp: f32,
    phase: f32,
}

/// Procedural walk/run cycle: legs and arms swing in opposition, scaled by ground speed.
pub fn animate_soldiers(
    time: Res<Time>,
    mut roots: Query<(&Transform, &mut Gait)>,
    mut limbs: Query<(&Limb, &ChildOf, &mut Transform), Without<Gait>>,
) {
    let dt = time.delta_secs().max(1e-4);
    for (t, mut g) in &mut roots {
        let p = t.translation;
        let d = Vec3::new(p.x - g.last.x, 0.0, p.z - g.last.z);
        let raw = if g.primed && d.length() < 3.0 { (d.length() / dt).min(14.0) } else { 0.0 };
        g.last = p;
        g.primed = true;
        let k = 1.0 - (-12.0 * dt).exp();
        g.speed += (raw - g.speed) * k;
        let target = (g.speed / 5.0).clamp(0.0, 1.0) * 0.8;
        g.amp += (target - g.amp) * k;
        g.phase = (g.phase + g.speed * 1.4 * dt) % std::f32::consts::TAU;
    }
    for (limb, parent, mut tr) in &mut limbs {
        let Ok((_, g)) = roots.get(parent.parent()) else { continue };
        // Opposite sides swing in opposition; each arm opposes the leg on its side.
        let sign = if limb.leg { limb.side } else { -limb.side };
        // The right arm carries the rifle, so it only sways a little.
        let scale = if limb.leg { 1.0 } else if limb.side > 0.0 { 0.25 } else { 0.7 };
        tr.rotation = Quat::from_rotation_x((g.phase.sin() * sign) * g.amp * scale);
    }
}

/// Where a soldier holds its weapon; the gun model is (re)built under it.
#[derive(Component, Default)]
pub struct HeldWeapon {
    pub gun: Option<Entity>,
    pub sig: Option<(String, u32)>,
}

#[derive(SystemParam)]
pub struct Build<'w, 's> {
    pub commands: Commands<'w, 's>,
    pub materials: ResMut<'w, Assets<StandardMaterial>>,
    pub art: ResMut<'w, Art>,
}

impl Build<'_, '_> {
    pub fn mat(&mut self, c: u32) -> Handle<StandardMaterial> {
        self.mat_kind(c, 0)
    }

    /// 0 = matte, 1 = glowing, 2 = translucent glass, 3 = unlit-ish water.
    pub fn mat_kind(&mut self, c: u32, kind: u8) -> Handle<StandardMaterial> {
        if let Some(h) = self.art.mats.get(&(c, kind)) {
            return h.clone();
        }
        let color = crate::state::rgb(c);
        let m = match kind {
            1 => StandardMaterial {
                base_color: color,
                emissive: color.to_linear() * 1.6,
                ..default()
            },
            2 => StandardMaterial {
                base_color: color.with_alpha(0.55),
                alpha_mode: AlphaMode::Blend,
                perceptual_roughness: 0.1,
                ..default()
            },
            3 => StandardMaterial { base_color: color, perceptual_roughness: 0.15, reflectance: 0.6, ..default() },
            _ => StandardMaterial { base_color: color, perceptual_roughness: 0.9, ..default() },
        };
        let h = self.materials.add(m);
        self.art.mats.insert((c, kind), h.clone());
        h
    }

    pub fn glow(&mut self, c: u32) -> Handle<StandardMaterial> {
        self.mat_kind(c, 1)
    }

    pub fn boxp(&mut self, parent: Entity, c: u32, center: Vec3, size: Vec3) -> Entity {
        let m = self.mat(c);
        self.boxm(parent, m, center, size)
    }

    pub fn boxm(&mut self, parent: Entity, m: Handle<StandardMaterial>, center: Vec3, size: Vec3) -> Entity {
        let mesh = self.art.cube.clone();
        let e = self
            .commands
            .spawn((Mesh3d(mesh), MeshMaterial3d(m), Transform::from_translation(center).with_scale(size)))
            .id();
        self.commands.entity(parent).add_child(e);
        e
    }

    pub fn boxr(&mut self, parent: Entity, c: u32, center: Vec3, size: Vec3, rot: Quat) -> Entity {
        let e = self.boxp(parent, c, center, size);
        self.commands.entity(e).insert(Transform { translation: center, rotation: rot, scale: size });
        e
    }

    /// Cylinder along local Y unless rotated.
    pub fn cyl(&mut self, parent: Entity, c: u32, center: Vec3, radius: f32, height: f32, rot: Quat) -> Entity {
        let mesh = self.art.cylinder.clone();
        let m = self.mat(c);
        let e = self
            .commands
            .spawn((
                Mesh3d(mesh),
                MeshMaterial3d(m),
                Transform { translation: center, rotation: rot, scale: Vec3::new(radius * 2.0, height, radius * 2.0) },
            ))
            .id();
        self.commands.entity(parent).add_child(e);
        e
    }

    pub fn cone(&mut self, parent: Entity, c: u32, center: Vec3, radius: f32, height: f32) -> Entity {
        let mesh = self.art.cone.clone();
        let m = self.mat(c);
        let e = self
            .commands
            .spawn((
                Mesh3d(mesh),
                MeshMaterial3d(m),
                Transform::from_translation(center).with_scale(Vec3::new(radius * 2.0, height, radius * 2.0)),
            ))
            .id();
        self.commands.entity(parent).add_child(e);
        e
    }

    #[allow(dead_code)]
    pub fn sphere(&mut self, parent: Entity, c: u32, center: Vec3, radius: f32) -> Entity {
        let mesh = self.art.sphere.clone();
        let m = self.mat(c);
        let e = self
            .commands
            .spawn((Mesh3d(mesh), MeshMaterial3d(m), Transform::from_translation(center).with_scale(Vec3::splat(radius * 2.0))))
            .id();
        self.commands.entity(parent).add_child(e);
        e
    }

    /// Blocky soldier, root at body centre (feet at y = -0.9), facing +Z.
    pub fn soldier(&mut self, root: Entity, team: u32) {
        use pal::*;
        let v = Vec3::new;
        for s in [-1.0f32, 1.0] {
            // Legs swing from the hip, arms from the shoulder.
            let leg = self.pivot(root, v(0.13 * s, -0.17, 0.0), Limb { leg: true, side: s });
            self.boxp(leg, BOOT, v(0.0, -0.66, 0.03), v(0.22, 0.15, 0.32));
            self.boxp(leg, OLIVE, v(0.0, -0.30, 0.0), v(0.22, 0.6, 0.24));
            self.boxp(leg, team, v(0.0, -0.33, 0.125), v(0.2, 0.12, 0.04));
            let arm = self.pivot(root, v(0.4 * s, 0.42, 0.0), Limb { leg: false, side: s });
            self.boxp(arm, team, v(-0.01 * s, 0.0, 0.0), v(0.2, 0.1, 0.28));
            self.boxp(arm, OLIVE, v(0.0, -0.29, 0.0), v(0.18, 0.55, 0.2));
            self.boxp(arm, team, v(0.0, -0.54, 0.0), v(0.19, 0.06, 0.21));
            self.boxp(arm, OLIVE_DARK, v(0.0, -0.64, 0.0), v(0.17, 0.12, 0.18));
            self.boxp(root, BLACK, v(0.08 * s, 0.68, 0.172), v(0.06, 0.05, 0.01));
        }
        self.boxp(root, OLIVE, v(0.0, 0.15, 0.0), v(0.6, 0.6, 0.3));
        self.boxp(root, OLIVE_DARK, v(0.0, 0.2, 0.0), v(0.62, 0.42, 0.36));
        self.boxp(root, KHAKI, v(0.0, 0.12, 0.185), v(0.42, 0.14, 0.02));
        self.boxp(root, BOOT, v(0.0, -0.13, 0.0), v(0.62, 0.08, 0.34));
        self.boxp(root, SKIN, v(0.0, 0.66, 0.0), v(0.34, 0.34, 0.33));
        self.boxp(root, OLIVE, v(0.0, 0.86, -0.01), v(0.42, 0.2, 0.42));
        self.boxp(root, team, v(0.0, 0.87, -0.01), v(0.09, 0.21, 0.43));
        let gun = self
            .commands
            .spawn((Transform::from_xyz(0.24, 0.06, 0.3).with_scale(Vec3::splat(1.15)), Visibility::default(), HeldWeapon::default()))
            .id();
        self.commands.entity(root).add_child(gun);
    }

    fn pivot(&mut self, parent: Entity, at: Vec3, limb: Limb) -> Entity {
        let e = self.commands.spawn((Transform::from_translation(at), Visibility::default(), limb)).id();
        self.commands.entity(parent).add_child(e);
        e
    }

    /// Vehicle model centred on its collider (half extents `he`), facing +Z.
    pub fn vehicle(&mut self, root: Entity, def: &str, he: Vec3, accent: u32) {
        use pal::*;
        let v = Vec3::new;
        let side = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        match def {
            "jeep" => {
                self.boxp(root, OLIVE, v(0.0, -0.05, 0.0), v(2.0, 0.55, 4.2));
                self.boxp(root, KHAKI, v(0.0, 0.3, 1.45), v(1.9, 0.3, 1.3));
                self.boxp(root, KHAKI, v(0.6, 0.0, -0.8), v(0.82, 0.57, 1.2));
                self.boxp(root, BLACK, v(0.0, 0.25, 2.12), v(1.2, 0.35, 0.05));
                let glass = self.mat_kind(GLASS, 2);
                self.boxm(root, glass, v(0.0, 0.75, 0.75), v(1.8, 0.6, 0.06));
                for x in [-0.9, 0.9] {
                    self.boxp(root, BOOT, v(x, 1.0, -0.6), v(0.08, 1.0, 0.08));
                    self.boxp(root, BOOT, v(x, 1.0, 0.75), v(0.08, 0.5, 0.08));
                    self.boxp(root, BOOT, v(x, 1.48, 0.07), v(0.08, 0.08, 1.4));
                }
                for (x, z) in [(-0.4, 0.2), (0.4, 0.2), (-0.4, -1.1), (0.4, -1.1)] {
                    self.boxp(root, OLIVE_DARK, v(x, 0.35, z), v(0.6, 0.6, 0.5));
                }
                self.cyl(root, BLACK, v(0.0, 0.5, -2.2), 0.42, 0.3, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2));
                for (x, z) in [(-1.05, 1.35), (1.05, 1.35), (-1.05, -1.35), (1.05, -1.35)] {
                    self.cyl(root, BLACK, v(x, -0.4, z), 0.45, 0.35, side);
                    self.cyl(root, KHAKI, v(x * 1.05, -0.4, z), 0.2, 0.36, side);
                }
                self.boxp(root, accent, v(0.0, 0.47, 1.45), v(0.4, 0.02, 0.6));
            }
            "tank" => {
                self.boxp(root, OLIVE, v(0.0, 0.0, 0.0), v(2.6, 0.9, 6.0));
                self.boxp(root, KHAKI, v(0.0, 0.2, 2.6), v(2.4, 0.5, 1.0));
                for x in [-1.45, 1.45] {
                    self.boxp(root, BLACK, v(x, -0.55, 0.0), v(0.55, 0.9, 6.5));
                    for i in 0..6 {
                        self.cyl(root, KHAKI, v(x * 1.08, -0.6, -2.6 + i as f32 * 1.04), 0.33, 0.12, side);
                    }
                    self.boxp(root, OLIVE_DARK, v(x, 0.0, 0.0), v(0.6, 0.12, 6.4));
                }
                let turret = self
                    .commands
                    .spawn((Transform::from_xyz(0.0, 0.85, -0.3), Visibility::default(), TurretPart))
                    .id();
                self.commands.entity(root).add_child(turret);
                self.boxp(turret, OLIVE, v(0.0, 0.0, 0.0), v(2.0, 0.8, 2.8));
                self.boxp(turret, KHAKI, v(0.0, 0.0, -1.5), v(1.6, 0.6, 0.8));
                self.boxp(turret, accent, v(0.0, 0.42, 0.0), v(0.5, 0.04, 0.5));
                self.cyl(turret, OLIVE_DARK, v(0.5, 0.55, -0.4), 0.25, 0.3, Quat::IDENTITY);
                self.boxp(turret, BLACK, v(-0.7, 1.0, -1.1), v(0.03, 1.4, 0.03));
                let barrel = self
                    .commands
                    .spawn((Transform::from_xyz(0.0, 0.0, 1.3), Visibility::default(), BarrelPart))
                    .id();
                self.commands.entity(turret).add_child(barrel);
                self.cyl(barrel, OLIVE_DARK, v(0.0, 0.0, 1.9), 0.13, 3.8, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2));
                self.cyl(barrel, OLIVE_DARK, v(0.0, 0.0, 3.8), 0.18, 0.5, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2));
            }
            "heli" => {
                self.boxp(root, OLIVE, v(0.0, -0.1, 0.3), v(2.2, 2.0, 6.6));
                self.boxp(root, KHAKI, v(0.0, -0.1, 3.8), v(1.9, 1.6, 0.8));
                let glass = self.mat_kind(GLASS, 2);
                self.boxm(root, glass, v(0.0, 0.25, 4.15), v(1.7, 0.8, 0.1));
                self.boxp(root, OLIVE_DARK, v(0.0, 1.05, 3.3), v(1.0, 0.5, 1.2));
                self.boxp(root, OLIVE_DARK, v(0.0, 1.0, -3.1), v(1.3, 1.6, 1.6));
                for z in [-1.4, 0.0, 1.4] {
                    for x in [-1.11, 1.11] {
                        self.boxp(root, BLACK, v(x, 0.25, z), v(0.02, 0.45, 0.6));
                    }
                }
                self.boxp(root, accent, v(0.0, -0.1, 0.3), v(2.22, 0.15, 1.0));
                for (x, z) in [(-0.9, 2.6), (0.9, 2.6), (-0.9, -2.4), (0.9, -2.4)] {
                    self.cyl(root, BLACK, v(x, -1.25, z), 0.22, 0.2, side);
                }
                for (y, z) in [(1.45, 3.3), (1.95, -3.1)] {
                    self.cyl(root, BOOT, v(0.0, y - 0.15, z), 0.12, 0.4, Quat::IDENTITY);
                    let rotor = self
                        .commands
                        .spawn((Transform::from_xyz(0.0, y, z), Visibility::default(), Spinner { axis: Vec3::Y, speed: 0.0 }))
                        .id();
                    self.commands.entity(root).add_child(rotor);
                    self.boxp(rotor, BOOT, Vec3::ZERO, v(8.0, 0.05, 0.3));
                    self.boxp(rotor, BOOT, Vec3::ZERO, v(0.3, 0.05, 8.0));
                }
            }
            "plane" => {
                self.boxp(root, OLIVE, v(0.0, 0.1, 0.2), v(1.3, 1.3, 7.0));
                self.boxp(root, KHAKI, v(0.0, 0.1, 1.0), v(1.32, 1.32, 1.0));
                self.boxp(root, KHAKI, v(0.0, 0.1, -1.6), v(1.32, 1.32, 1.0));
                let glass = self.mat_kind(GLASS, 2);
                self.boxm(root, glass, v(0.0, 0.85, 0.3), v(0.8, 0.5, 1.5));
                self.boxp(root, OLIVE, v(0.0, -0.15, 0.6), v(9.0, 0.15, 1.8));
                self.boxp(root, KHAKI, v(3.0, -0.14, 0.6), v(1.4, 0.16, 1.82));
                self.boxp(root, KHAKI, v(-3.0, -0.14, 0.6), v(1.4, 0.16, 1.82));
                self.boxp(root, accent, v(4.0, -0.13, 0.6), v(0.6, 0.17, 1.83));
                self.boxp(root, accent, v(-4.0, -0.13, 0.6), v(0.6, 0.17, 1.83));
                self.boxp(root, OLIVE, v(0.0, 0.3, -3.3), v(3.2, 0.12, 1.0));
                self.boxp(root, OLIVE, v(0.0, 1.0, -3.3), v(0.12, 1.4, 1.1));
                self.cyl(root, BOOT, v(0.0, 0.1, 3.85), 0.35, 0.3, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2));
                let prop = self
                    .commands
                    .spawn((Transform::from_xyz(0.0, 0.1, 4.05), Visibility::default(), Spinner { axis: Vec3::Z, speed: 0.0 }))
                    .id();
                self.commands.entity(root).add_child(prop);
                self.boxp(prop, BOOT, Vec3::ZERO, v(0.2, 2.6, 0.06));
                self.boxp(prop, HAZARD, v(0.0, 1.25, 0.0), v(0.21, 0.15, 0.07));
                self.boxp(prop, HAZARD, v(0.0, -1.25, 0.0), v(0.21, 0.15, 0.07));
                for x in [-1.3, 1.3] {
                    self.boxp(root, BOOT, v(x, -0.55, 1.0), v(0.1, 0.8, 0.1));
                    self.cyl(root, BLACK, v(x, -0.9, 1.0), 0.18, 0.15, side);
                }
            }
            _ => {
                self.boxp(root, WOOD, Vec3::ZERO, he * 2.0);
                self.boxp(root, KHAKI, Vec3::ZERO, he * 2.02 * v(1.0, 0.2, 1.0));
            }
        }
    }

    pub fn tree(&mut self, root: Entity, scale: f32) {
        self.cyl(root, pal::WOOD, Vec3::new(0.0, 1.0 * scale, 0.0), 0.25 * scale, 2.0 * scale, Quat::IDENTITY);
        self.cone(root, pal::PINE, Vec3::new(0.0, 3.2 * scale, 0.0), 1.8 * scale, 3.2 * scale);
        self.cone(root, pal::PINE, Vec3::new(0.0, 4.8 * scale, 0.0), 1.3 * scale, 2.6 * scale);
    }
}

pub fn spin_parts(time: Res<Time>, mut q: Query<(&Spinner, &mut Transform)>) {
    let dt = time.delta_secs();
    for (s, mut t) in &mut q {
        if s.speed != 0.0 {
            t.rotate_local(Quat::from_axis_angle(s.axis, s.speed * dt));
        }
    }
}
